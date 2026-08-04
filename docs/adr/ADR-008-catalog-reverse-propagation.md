# ADR-008：增量模型工作计划与目录反向传播

状态：Accepted（2026-07-24）

## 决策

- 生成根统一由 `generation_root` 决定：最近 MDU 优先，否则使用 Normal Granularity 的 significant owner。
- 每个增量文件在 PE 落库前建立模型工作计划；PE 成功后先写入 `model_update_pending`，再推进 `applied_sesno`。
- `ref_rev` 继续作为 `referenced -> referrer` 的反向索引。共享目录/规格变更通过该索引把引用者加入生成根计划；查询失败时持久化 `cascade_expand` 种子，成功重查后先幂等写入派生根任务再删除种子。索引维护失败仍只告警，不阻断数据水位。
- `cascade_expand` 的延迟重查采用去重、防环、直到 frontier 为空的无深度上限遍历；不得用固定 hop 上限把非空 frontier 当作成功。查询失败保留原种子整次重试，本期不持久化中间 frontier，只有实测单次遍历规模或时长不可接受时才引入 continuation。
- 模型工作支持 `regen_root`、`transform`、`delete_cleanup`、`cascade_expand` 四类动作。任务身份已由 ADR-015 修订为 `(action, target_refno)`；`dbnum` 只作权威归属与路由字段，来源会话只作追踪，任务新鲜度与收口由队列内部 revision 决定。执行采用 at-least-once 语义：模型失败不回滚数据或水位，任务保留重试。

## 后果

- 自动、手动和补偿路径共享相同根规则；移动仍同时覆盖旧根和新根。
- 水位后的进程崩溃只会留下可消费任务，不会丢失模型更新。
- 反向索引查询失败仍可能暂时漏掉级联，沿用 ADR-003 的非致命降级策略；保留的 `cascade_expand` 种子会在重试成功后恢复。
- 深层目录/规格引用链在一次成功的 `cascade_expand` 中必须完整收敛；代价是极端大图的一次执行可能较长，先接受这个上限，避免为尚未出现的规模问题维护持久化遍历状态。
