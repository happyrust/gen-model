# ADR-008：增量模型工作计划与目录反向传播

状态：Accepted（2026-07-24）

## 决策

- 生成根统一由 `generation_root` 决定：最近 MDU 优先，否则使用 Normal Granularity 的 significant owner。
- 每个增量文件在 PE 落库前建立模型工作计划；PE 成功后先写入 `model_update_pending`，再推进 `applied_sesno`。
- `ref_rev` 继续作为 `referenced -> referrer` 的反向索引。共享目录/规格变更通过该索引把引用者加入生成根计划；查询失败时持久化 `cascade_expand` 种子，成功重查后先幂等写入派生根任务再删除种子。索引维护失败仍只告警，不阻断数据水位。
- 模型工作按 `(dbnum, action, target_refno)` 去重，支持 `regen_root`、`transform`、`delete_cleanup`、`cascade_expand` 四类动作。执行采用 at-least-once 语义：模型失败不回滚数据或水位，任务保留重试。
- 旧 `incr_side_effect_pending:model_refresh` 和 `manual_model_pending` 在消费时惰性转换为新任务；不做启动时批量迁移。

## 后果

- 自动、手动和补偿路径共享相同根规则；移动仍同时覆盖旧根和新根。
- 水位后的进程崩溃只会留下可消费任务，不会丢失模型更新。
- 反向索引查询失败仍可能暂时漏掉级联，沿用 ADR-003 的非致命降级策略；下一次触及或旧任务转换可恢复。
