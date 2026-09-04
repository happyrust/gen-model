# ADR-050：`model_update_pending` 只属于当前进程

- 状态：Accepted
- 日期：2026-08-25
- 关联：ADR-001（水位是提交承诺）、ADR-011（唯一批次队列）、ADR-048（监听限定域上弦）

## 背景

`model_update_pending` 曾被当成跨重启的持久恢复队列，启动空闲轮会继续消费上一进程
留下的模型工作单。这与真实提交边界不一致：数据窗口直写 RocksDB 并在尾事务里同时落下
应用水位与模型窗口意图（ADR-056），模型工作单只是**本进程**在那次提交之上排出的生成
计划；进程结束后，旧工作单不再对应新进程启动时看到的水位、`gen_root` 凭证与文件最新
会话（ADR-054），重放它们会把两个进程的状态混合。

> 2026-09-03 按 ADR-056「后果」改写本段措辞（原文以 kv-mem 暂存快照解释提交边界）；决策本体不变。

## 决策

1. `model_update_pending` 是**本进程调度账本**，不是跨重启恢复日志。
2. `aios-database` 获取单实例锁并连接 SurrealDB 后，必须立即全量删除该表；删除发生在
   空间树装载、surql 预加载、db manager 构造、watcher、worker、全量或增量模型生成前。
3. 清理失败必须阻断启动，不得打印告警后继续。继续运行会让旧快照进入新提交链。
4. 启动日志必须报告实际删除行数，使“为什么没有继续跑历史模型任务”可直接观察。
5. ADR-048 的 `watch_dbnums` 谓词保留，作为本进程运行期间的第二道范围防线；它不再
   承担跨重启保存范围外工作单的职责。
6. 本决策只覆盖 `model_update_pending`。增量水位、提交尝试和其他有独立恢复协议的表
   不随之清理。

## 结果

- 重启不会继续上一进程的模型生成；新任务只能由本次解析/人工触发重新产生。
- 启动顺序与清理失败语义由内存 SurrealDB 回归测试和 `run_app` 源码顺序测试共同钉住。
- 回滚只恢复旧启动语义，不改 RocksDB 中已经提交的数据与模型。

## 补充（2026-08-26）

清空是无条件的，补偿也必须是：`run_cli` 数据基线就绪后由
`model_update_pending::reconcile_model_coverage_at_startup` 做启动覆盖核对——
监听限定域逐库 `fn::sync_gen_roots` + 补种；未限定域按 `gen_root` 凭证点查、
发现过期才对该库完整 sync + 补种，没有凭证记录的库当面播报「核不了」。
核对或补种失败只告警不阻断启动，该库按「覆盖未确认」处理。此前补偿只挂在
监听限定域上，未限定域进程在模型积压期间重启会把缺口静默判成「模型完整」。

## 修订（2026-09-03）

1. **清空范围收窄为五种数据动作。** 决策 2 的「全量删除该表」改为「全量删除
   `DATA_ACTION_FILTER` 那五种（transform / delete_cleanup / cascade_expand /
   regen_root / post_regen_aabb），不分库、含 `dbnum = 0` 的按需行」；
   `room_recalc_*` 保留。理由：五种数据动作都能由启动复核从 `gen_root` 凭证重新
   排出来，房间任务没有这条补偿路径——它唯一的补偿是启动整库房间重建，而那一步在
   direct / env 禁用 / `autorun=false` / `model_incremental=false` 下都不跑，这些形态下
   把房间行一起删掉就是真丢工作且无任何报错。与整库重建作废
   （`discard_pending_for_full_rebuild`）同一口径；启动日志对保留的房间行数当面播报。
   （审核 P2，计划 `docs/plans/2026-09-03-startup-orchestration-p1-p3-fix-plan.md` T3。）
2. **补偿实现换代。** 上文的「`fn::sync_gen_roots` + 补种」已被 ADR-058 D4/D5 取代：
   `startup_coverage_floors()` 一次聚合取各库安定凭证下限，落后的库开一次退化窗口
   （`reconcile_startup_window`，`S` = 凭证下限、`T` = 文件最新会话），选根与稳态共用
   `WindowRootPlan`。「清空无条件、补偿也无条件」的原则不变。
3. **补偿执行点改为 `run_cli` 顶层无条件。** 此前它被 `startup_data_ready` 把门，
   `startup_autorun=false` 且未声明限定域时补偿根本不执行，与本节第一句话直接矛盾。
   现无条件执行；`!armed` 形态下复核只入队不消费（行由清单与上弦门控）。（审核 P2，T4。）
4. **python `full_init` 补齐同一道清空。** 它此前不清表却由 `drain_data` 消费同一张表；
   现连接成功后立即调 `clear_stale_at_process_start()`，失败按决策 3 上抛。（审核 P4，T9。）
