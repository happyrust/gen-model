# ADR-048：声明监听限定域即为启动上弦

- 状态：Accepted
- 日期：2026-08-25
- 关联：ADR-011（唯一批次队列）、ADR-023（启动自愈与数据支撑）、ADR-033（增量阶段独立执行控制）、ADR-034（监听限定域的 CATA 依赖闭包）

## 背景

进程起手要不要自动干活，此前只由 `startup_autorun` 一个开关决定
（`batch_scheduler.rs` 的 `auto_work_armed` 初值）。它管两侧：

- 批次侧：重扫排出来的行是否挂起（`increment_manager::sweep_holds_rows`）；
- 空闲轮侧：`model_update_pending` 这张持久表的积压是否消化
  （`batch_worker::run_batch_worker` 的空闲轮门）。

关着时的解封条件是「某个 dbnum 真的来一次增量」——文件事件或人工执行。

`watch_dbnums` / `--watch-dbnum`（ADR-034 那条线）是另一个开关，只做一件事：
把增量摄入**收窄**到指定 dbnum。两个开关此前互不知道对方存在。

现场（2026-08-25，`db_options/DbOption-rvm-rebuild.toml` + `AIOS_STARTUP_AUTORUN=0`）
把这个缝暴露成了一个死局：

```
startup_autorun = false        →  重扫行挂起、持久积压不消化
sync_live       = false        →  watcher 压根没起
```

于是解封条件**永远不会发生**。`/health` 上 `model_update_pending.retryable = 7655`
（regen_root 7267 + room_recalc 388）纹丝不动，进程 33 分钟只吃掉 74.9 秒 CPU
（单核 2.1%），`model_drain.last_claimed_epoch = null`——消费器一次都没领过活。
按宪法「队列里每一行都要有三条明确出路」的口径，这 7655 行**一条都没有**：
不可消费、不可收口、也不会复活。

而它对外的样子是「在慢慢跑」，不是「停了」。这正是本仓定义的最高级别缺陷：静默失效。

## 决策

1. **起手上弦有两个来源，任一成立即上弦**，实现收在一个纯函数里
   （`batch_scheduler::initial_auto_work_armed`）：

   ```rust
   pub(crate) fn initial_auto_work_armed(startup_autorun: bool, watch_scope_active: bool) -> bool {
       startup_autorun || watch_scope_active
   }
   ```

   理由：`startup_autorun=false` 的本意是「别自作主张把**整库**都跑了」，
   而手写 `watch_dbnums = [1112, 8000]` 已经是一句**比它更窄、更明确**的
   「本次跑就要这几个库」。让后者继续等前者点头，等的是一个自己已经回答过的问题。

2. **限定域仍然只收窄，不放宽**。上弦只决定「要不要开工」，开哪些库照旧由
   `watch_scope::admits` 裁决——限定域外的库该跳过还是跳过，理由措辞不变
   （ADR-034 的三句无交集约束继续成立）。

   这条约束同时覆盖 `model_update_pending` 的**全局自动消费**，不能只覆盖文件重扫：
   自动 drain、是否还有活、死信阻断与 `/health` 的模型就绪口径都必须追加同一条
   `dbnum IN watch_dbnums` 谓词。这里约束的是**本进程新建**的工作单；根据后续
   ADR-050，上一进程遗留的整张表会在任何生产者/消费者启动前清空，不跨重启接手。
   当前批次提交后按精确任务键执行的 scoped drain 不改，它本来就没有
   扫描全表、也不会捎进隔壁库。

3. **不覆盖启动全量房间重建**。`lib.rs::skip_startup_room_build` 的第二道门直读
   `startup_autorun()`，与上弦位无关。收窄到几个库的人要的正是「别为 2 万面板的
   全量重建付那十几秒」，让限定域把它一起拖回来与限定域的字面意思相反。

4. **两个来源都不成立时，那句启动播报要把话说完**。此前只说
   `startup_autorun=false`，现在还要点名「未声明 watch_dbnums」这条出路；
   并且当 `sync_live=false` 一并成立时，必须额外喊一句「不会有文件事件来解封任何
   一行」——解封条件不可能满足这件事，不能让人自己去两份配置里对出来。

5. 手动触发与 watcher 继续共用 ADR-011 的唯一队列与同一组门，本决策不新增消费路径，
   也不新增第二处「哪些库在范围内」的判定。

## 结果

- 写了 `watch_dbnums` 的部署，启动即消化这几个库的批次与持久积压，不必再等一次
  假想的文件事件；`AIOS_STARTUP_AUTORUN=0` 保留原义（不写限定域时行为逐位不变）。
- 本进程限定域外的 `model_update_pending` 不再被空闲轮捎带执行，也不再让本次限定运行
  永久停在 `model_ready=false`；只读待重试清单展示本进程当前仍在册的行。
- 「配置里明明写了限定域，进程却什么都不干」这一格消失。真要「起来先看看」，
  就是两个开关都不给——那时的播报会把两条出路和 watcher 的缺席一起说清楚。
- 回归钉：
  - `batch_scheduler::a_declared_watch_scope_arms_the_process_just_like_autorun_does`
    —— 真值表，回退成单来源即红。
  - `batch_scheduler::the_global_scheduler_feeds_both_arming_sources_in`
    —— 源码断言，防止纯函数在位但实参被写死。
  - `lib.rs::the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild`
    —— 守住决策 3 的边界。
