# Implementation Plan：严格分阶段数据初始化

## Constitution Check

- **水位承诺**：阶段完成读取水位与数据支撑，不新增水位写路径。
- **单一规则**：候选裁决、manifest 与派发屏障供 watcher、手动和全量入口共用。
- **响亮失败**：候选不可读、重复和阶段失败均成为 blocker，不以 continue 伪装成功。
- **队列收口**：保留一个队列/派发器；模型意图继续使用持久补偿队列。
- **标识真值**：跨项目裸 dbnum 冲突由显式项目优先级裁决，不猜项目。
- **可执行守护**：阶段排序、epoch、并发、模型门和崩溃恢复均有测试。

无宪法例外。阶段状态保持可重建，避免新增第二完成真值。

## Design

1. 新增 `initialization_phase` 模块，定义 `DataPhase`、manifest、阶段快照、项目优先级裁决
   与全局协调器。
2. `batch_queue`/`batch_scheduler` 给批次附加 phase/epoch；派发谓词只选择协调器当前阶段，
   真实触发释放整个 manifest。
3. `increment_manager` 先收集完整候选再裁决、观察与入队；CATA 进入执行范围；阶段转换与
   watcher 事件都回到同一重扫入口。
4. `batch_worker` 在完成/失败时更新协调器，阶段 epoch 内只落 durable 模型意图；模型 drain
   受 `data_ready` 门控。
5. `sync_pdms` 分拆 Meta/CATA/DESI 三段；启动全量生成、按需生成与房间入口接入模型门。
6. REST/Python/health 增加只读阶段字段；配置增加 `catalogue_project_priority`。

## Verification

- 纯函数与调度器单测覆盖所有阶段和冲突分支。
- CI 特性口径的目标库单测、四个集成测试、Python offline 测试。
- SurrealDB 2.x 沙箱运行六条 live 场景，证据写入 `docs/evidence/` 和 live ledger。
- `cargo fmt --all -- --check`、`cargo check`、`sigmap verify-plan`、`sigmap review-pr`。

