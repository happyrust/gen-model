# Feature Specification：模型实例保存有界合批

## User Stories

### US1：小尾批低开销保存

模型生成器连续产出多个小 `ShapeInstancesData` 时，系统应在固定内存和固定等待上限内合批，
一次完成元数据解析和确定性保存，而不是每个 1～3 行尾批都独立访问数据库。

### US2：失败前不破坏旧模型

任何 NaN、normal/tubi 身份重叠或同一持久化 ID 的内容冲突，必须在首次删除前被发现并上抛；
失败批次不得被记为已产出，也不得触发陈旧模型清理或暂存窗口提交。

### US3：定向与全量行为一致

定向、手动、启动和整库生成必须共用同一保存器、同一去重规则和同一统计口径，同时保留
定向路径的 scoped cascade delete 与全量路径不预删的既有差异。

## Functional Requirements

- **FR-001**：receiver 必须保持单 consumer 和现有有界 flume 背压。
- **FR-002**：合批不得调用 `ShapeInstancesData::merge` 或 `merge_ref`，必须保留全部原始批。
- **FR-003**：`SavePlan` 必须在第一次删除前完成校验、元数据解析、去重、排序和 SQL 分包。
- **FR-004**：相同 record ID + 相同内容去重；相同 ID + 不同内容返回 typed conflict。
- **FR-005**：LCylinder 与非切角 SCylinder 的共享单位圆柱 ID 必须规范成单一参数值；其他
  同 geo hash 异参冲突。
- **FR-006**：neg relation 保持原顺序和索引；NGMR 只按完整三元组去重。
- **FR-007**：暂存模式串行执行 packet；直写模式最多四个 in-flight，首错后停止派发并收口。
- **FR-008**：只有成功 `SaveOutcome` 中的 refno 可计入本轮产出。
- **FR-009**：输出每轮 source batch、flush、实例/几何、等待、元数据查询、SQL packet/字节、
  scoped delete、冲突和 producer 阻塞统计；不再逐尾批打印 `Insert manual shape insts`。
- **FR-010**：不新增配置项、HTTP API、数据库表，不批量化 scoped delete，不删除共享 `inst_geo`。

## Success Criteria

- 固定 16 根、16～32 个 1～3 行源批夹具中，flush/save 次数与非删除 SQL packet 数均比逐批
  基线减少至少 70%；scoped delete 不计入该百分比。
- 同一输入任意排列均得到相同 canonical `SavePlan`、SQL 顺序与数据库终态。
- 保存阶段 p95、五轮端到端中位数和峰值内存不高于基线。
- staged 重放两次终态不变，提交前持久层保持旧显示；任一阶段失败不推进水位、不清 stale 行。

## Assumptions

- 所有生成路径整体切换，无运行时开关。
- 模型生成、mesh、布尔运算和生成根分页策略保持不变。
- 保存器只改变实例产物的聚合与写入调度，不改变 ADR-017 的提交单元边界。
