# Plan 018：房间增量失效闭环

## Constitution Check

- 房间任务、恢复信号与水位保持同事务；失败窗口不推进水位。
- watcher 与手动路径继续复用单一计划、暂存和 pending 消费链。
- 降级只用于可重建的结构枚举，并留下持久且可观察的重建要求。
- 不增加 SurrealDB 表或第二条队列消费路径。

## 实施

1. 将定向增量的实际几何目标保守接入 AABB/房间/epoch 原子链，并保留普通刷新只按 AABB 的口径。
2. 扩展海工结构触发器，覆盖 `CWALL/CFLOOR → PANE`。
3. 在计划和房间重建凭据中持久化结构枚举降级状态。
4. 增加离线、崩溃重放和 dbnum=8000 live 对拍。

四个 CI 集成目标使用默认净窗口依赖图；`db8000_two_delete_fixture` 与
`db8000_session_pairs` 不启用 `legacy_session_replay`，也不调用逐会话实体回放。

## Complexity Tracking

保守失效只覆盖定向增量实际目标；全量基线仍以一次全量房间重建收尾。结构枚举降级复用
`room_build:main`，不引入新状态表。
