# ADR-015：待重试模型工作以动作和目标标识

状态：Accepted（2026-07-29）

本 ADR 修订 ADR-008 中把 `dbnum` 放进任务键的决定。项目内 `target_refno` 的 Ref0 唯一归属一个 `dbnum`，因此 durable model work 统一以 `(action, target_refno)` 为唯一身份；`dbnum` 保留为可反查、可校验的路由字段，不参与记录 ID。

同一动作与目标再次入队却给出不同 `dbnum` 时，必须按 Ref0 库归属校验并报告冲突，不能创建第二条任务。冲突只阻断受影响 Ref0 的工作并保留其 pending，不阻断其他根。本方案按全新存储结构实现，不读取、转换或兼容旧 pending 记录。

并发新鲜度完全由队列内部单调 `revision` 表达：每次确认有新工作时递增，worker 领取时保存该值，成功删除或失败标记都必须以 revision 相等为条件。`source_dbnum` 与 `source_end_sesno` 只是最近触发来源的追踪字段，不参与跨库排序、去重或死信复活；死信只在确认有新工作入队或收到显式重试时复活。

显式重试只允许针对已存在的 `(action, target_refno)` pending：在一个原子更新中递增
`revision`、清零 `attempts`、清除 `last_error` 并恢复为 pending。它不能凭空创建工作，
也不提供批量复活；旧 worker 因 revision 不匹配不能结算被复活的行。

## Consequences

- 同一生成根不会因目录触发、设计触发或错误来源 dbnum 产生多条 regen pending。
- 生成根锁、模型生成结果和 regen pending 共享 refno-only 身份。
- 跨库会话号无需比较；即使不同来源触发同一目标，也只由本地 revision 防止旧 worker 错误收口。
- pending 存储从空状态启用，不需要迁移程序、双读或兼容分支。
- 所有动作共用一个最小显式重试入口，不为每种动作另建恢复 API。
