# 增量更新初始化执行过程审核（2026-08-17）

- 审核对象：ADR-025 严格初始化屏障下的增量执行链——`run_cli` 启动序
  （`src/lib.rs`）→ `init_watcher` 首轮重扫入队（`increment_manager::sweep_dirs`）→
  `batch_worker` 按 Meta → Catalogue → Design 派发消费 → 数据/水位收口 →
  模型门（`initialization_phase`）→ 房间。含 watch 事件、周期对账（reconcile）、
  scope-refresh、share-remount 四条重扫路径与手动执行路径的汇流点。
- 方法：静态源码审核 + 既有单测阅读。四项发现**当轮全部收口**并随
  `1ae4be04`（fix(data): freeze merge baselines at enqueue and harden
  initialization visibility）提交；全量 lib 单测 878 通过 0 失败
  （features `ws,gen_model,manifold,project_hd,http_api`），
  `cargo check --tests` 确认提交树自洽。
- 关联：ADR-025（严格阶段屏障）、ADR-011（单队列单派发器）、ADR-021（回退
  默认整库重建）、ADR-017（暂存窗口）；宪法「静默失效是最高级别缺陷」条。

---

## 0. 结论摘要

| # | 严重度 | 一句话 | 收口（当轮，`1ae4be04`） |
|---|---|---|---|
| P1 | 高 | 重扫读不出 DESI 最新会话号只 warn+跳过：清单缺库照样宣告 `data_ready`、模型门照开，库持续读不动时外面毫无痕迹；DICT/CATA 头不可读却是阻断 Meta 的（ADR-025 §6），同一种「观察不完整」两副面孔 | ✅ 读失败记进对应阶段 blockers，该阶段可见地不就绪；瞬态靠周期对账重扫（默认 300s）恢复即解。源码钉 `sweep_skips_always_leave_a_phase_blocker`（文件末尾、`concat!` 拼 marker） |
| P2a | 高 | `mark_failed` 按任务终态标签判定：数据 Applied 而模型侧失败的 Partial 也把数据阶段拉 Blocked——模型失败已有 durable pending 重试账 + 死信门槛扣模型门，再关数据门是双重惩罚，同阶段其余库连坐一个对账周期 | ✅ 判据改为数据窗口本身（`batch_failure_blocks_data_phase`，看 `batch.status`）：Applied/Skipped 不阻断；数据批次 Failed 折成的 Partial（有单元成功）照旧阻断；没跑到数据步（冻结重扫失败、预检失败）保守阻断。单测 `only_an_unsettled_data_window_blocks_the_data_phase` |
| P2b | 高 | 确定性失败无上限重跑：批次失败 → Blocked → 周期对账重扫装新 epoch → 水位没动再入队再跑，每 300s 一轮；坏文件/必现 panic 的大库一跑几十分钟，正常批次全排在后面，且没有任何计数 | ✅ 新增进程内连败账本 `BatchFailureLedger`（batch_worker）：同 dbnum 同右端连败到 `MAX_ATTEMPTS`（5）后重扫侧 park——不再自动入队、记阶段 blocker 保持可见；文件长出新会话（右端前进，查询内顺带清账）或人工执行（execute 入队点显式 `reset_batch_failure`）复活，成功/数据收口即清零，panic 路径记同一本账；`/health` 新增 `batch_failures`。单测钉 park / 复活 / 右端前进重数三条出路 |
| P3 | 低 | `restore_persisted_pause` 在 `run_cli` 与 `run_batch_worker` 各调一次，暂停时同一次启动打两条措辞不同的日志 | ✅ worker 侧静默化（独立入口兜底保留，失败仍出声），播报归 `run_cli` 一处 |
| P4 | 低 | 启动主线 `wait_for_model_ready` 干等：收敛在空闲轮里（模型积压分页、空间收敛、AABB 落盘，任一环失败按 30s 退避），主线长时间无输出像挂死 | ✅ 每 60s 播报一次仍在等什么、去哪看原因；`lib.rs` 既有源码钉子同步更新锚点 |

**一句话现状**：四项全部当轮收口并已提交；同 commit 一并落地此前未提交的
merged_sesnos 基线冻结批次（同文件交错、跨文件互依赖，拆不干净，body 里分条）。

---

## 1. 验证过的自洽性（审核正面结论）

以下闭环逐条对过代码与单测，成立：

- **三阶段屏障**：`epoch + manifest + allows + reconcile_pending` 闭环；阶段
  转换强制重扫（`needs_rescan` → 空闲轮 `resweep_for_scope_change`），旧 epoch
  的完成不满足新 manifest（`old_epoch_cannot_satisfy_new_manifest` 钉住）。
- **重扫不会孤立旧行**：整面重扫对每个仍在磁盘上的候选都会再发现，
  `batch_queue::enqueue` 合并时刷新行的 `epoch_id`/`phase`；AlreadyCovered 导致
  的 manifest/队列行不一致由 `finish → reconcile_pending → needs_rescan → resweep`
  收口。
- **held/arm 语义**：重扫行挂起不算队首、真实触发按 dbnum 放行并与积压合成
  一条（`a_real_trigger_releases_the_backlog_and_merges_it_into_one_run` 等钉住）；
  `startup_autorun=false` 时空闲轮持久积压整体等上弦信号。
- **F6 纪律**：判重先于 `record_scan`（源码顺序断言在位）；`previous_observed_sesno`
  基线在 `record_observation` 覆盖前冻结、合并只认最早观察。
- **冻结点**：`record_frozen_end` 回写真实右端、吸收后继行、后继行左端跟进；
  `Reinitialize` 控制意图不被数值覆盖销掉。
- **初始化批次让位**：`epoch>0` 的批次 `defer_model_phase`——数据与水位先全部
  收口，模型工作留待数据队列清空后空闲轮分页消费，大库几何不堵数据队列。
- **worker 生存性**：唯一消费者 + `isolate_panic` 逐批隔离 + `WorkerLiveGuard`
  放倒存活旗 + 空闲轮 panic 账本（同因连撞停跑、真跑过批次复活）。
- **恢复通道全景**（P2b 的前提）：失败后的自动重试来源 = watch 事件重扫、
  周期对账重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s，0 关闭）、SYS meta
  落库后的 scope-refresh、共享盘重挂补扫；全部汇入同一条
  `sweep_dirs → enqueue_discovered → begin_discovery/install_manifest` 路径。

## 2. 审核中修正过的认知（记录以免复查走弯路）

- 「DESI 头不可读静默」**不成立**：`catalogue_manifest_for_dirs` 对监控目录里
  **每个**候选文件都先过一遍头部解析，读不动记 Meta blocker——真正无痕的只有
  `get_latest_sesno` 读失败这半边（P1 修的就是它）。
- 「失败后永久停摆」**只在关掉周期对账时成立**：默认 300s 一轮的 reconcile
  重扫就是自动重试通道；P2b 补的是它缺的上限，不是缺的通道。
- 手动路径读失败不需要 blocker：`scan_project_candidates` 的 warning 进执行
  回执，发起人看得见；自动路径没有回执，可见性才必须走 manifest/health。

## 3. 遗留（未列为缺陷）

- 一个库的数据批次 Failed 仍会把**同阶段其余库**的派发挡住一个对账周期
  （`mark_failed` → status=Blocked → `allows` 全 false）。这是 ADR-025 §4「blocker
  关闭模型门、阶段不越过」的有意保守面；P2a/P2b 已把误伤面（Partial、确定性
  失败风暴）切掉，剩余行为符合「宁可阻断不可静默」。若未来要按 dbnum 细化
  阻断粒度，需先改 ADR-025。
- 启动房间段（`build_room_relations`）在 `open_model_phase` 返回 false 时仍会
  进入（`postprocess_allowed` 的 epoch-0 旁路兜底），失败仅告警不拦启动——
  房间是可重建派生数据，增量房间队列会再收敛，维持现状。
- live 半边未跑：本轮全部收口都有纯函数/源码钉，但连败 park 的端到端形态
  （真实库上连败 5 次 → /health `parked:true` → 保存新会话复活）未在 8019
  沙箱演练。要跑的话按 `docs/2026-08-12_live-test-ledger.md` 惯例补台账。
