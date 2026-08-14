# ADR-024：模型实例保存采用有界合批与先计划后修改

状态：Accepted（2026-08-14）

关联：ADR-012（合批重生成）、ADR-014（分支原子替换）、ADR-017（稳态增量窗口暂存写回）；`specs/005-shape-save-coalescing/`

## 背景

几何生产者按 worker / 分段发送 `ShapeInstancesData`。尾批通常只有 1～3 个实例，
而两个保存 receiver 每收到一批就立即执行一次完整保存：重复解析实例元数据、重新建立
去重映射，并生成多条 SQL / journal。稳态增量窗口内这些异步写最终还会争用同一个
`StagedExecutor` mutex，既没有获得真正并行，又支付了任务、锁和日志开销。

现有 `ShapeInstancesData::merge` / `merge_ref` 不携带全部关系字段，且 HashMap 覆盖会让
结果依赖输入顺序，不能作为可靠的合批语义。现有保存器还会在 transform 为 NaN 时静默
跳过，调用方却已把该 refno 计入本轮产出，可能导致陈旧模型清理建立在错误事实之上。

## 决策

1. 定向生成与整库生成共用单 consumer 的有界保存 receiver。receiver 按实例数、几何
   occurrence、源批数、估算字节数和等待时间合批；不增加第二条消费路径，不改变生产者
   channel 的有界背压。
2. 合批保存保留原始 `ShapeInstancesData` 列表，禁止调用现有 `merge` / `merge_ref`。
   在第一次 scoped delete 前先构建不可变 `SavePlan`：完成 NaN、normal/tubi 重叠、
   持久化 record ID 冲突、负关系顺序和共享几何参数的校验，再确定性去重、排序和分包。
3. `SaveMode` 显式区分 `TargetedReplace` 与 `FullBuild`。定向模式继续逐 refno 调用
   `delete_inst_relate_cascade`，不批量化删除、不删除共享 `inst_geo`；`inst_relate` 继续
   使用 ADR-014 的 delete+insert 事务。
4. 执行顺序固定为 scoped delete → 共享内容行 → 几何/负关系 → normal/tubi
   `inst_relate`。暂存模式按计划串行执行 SQL packet，避免伪并行；直写模式使用最多四个
   in-flight packet，并在首错后停止派发、等待已启动任务收口。
5. receiver 只根据成功 `SaveOutcome.written_refnos` 更新本轮产出。NaN、渲染或保存失败
   一律上抛；失败时调用方不会运行 stale prune，暂存窗口也不会提交。
6. 阈值为内部常量，不新增配置、HTTP API 或数据库表。本期保持模型生成、mesh、布尔运算
   与生成根分页策略不变。

## 参数与边界

- 软阈值：300 个实例行或 1200 个几何 occurrence。
- 硬阈值：1000 个实例行、4000 个几何 occurrence、32 个源批或 4 MiB 估算载荷。
- 空闲等待 2 ms；从首批开始的绝对等待上限 8 ms。
- SQL packet：最多 300 行且估算不超过 1 MiB。
- 达到硬阈值、channel 关闭或下一批放不下时立即 flush；软阈值到达后不再等待新批。

## 后果

- 小尾批共享一次元数据解析与一组确定性 SQL packet，暂存 journal / mutex 压力下降。
- 保存延迟最多增加 8 ms；硬阈值与有界 channel 将内存占用钉在固定上限。
- 同一 record ID 的不同渲染内容由 typed conflict 阻断，不再依赖 HashMap 遍历或任务完成顺序。
- 验收门槛：固定 16 根夹具中 save flush 与非删除 SQL packet 相较基线至少减少 70%，
  端到端耗时和峰值内存不得回退；scoped delete 单独统计。

## 否决方案

- 直接调用 `ShapeInstancesData::merge`：会遗漏 neg / NGMR 关系并产生顺序相关覆盖。
- 多 receiver / 多 save worker：破坏单 consumer 顺序并扩大同 ID 竞争。
- 单个巨型事务：与 ADR-017 已记录的 ws / 资源风险冲突。
- 合并 scoped delete 或回收 `inst_geo`：前者改变既有删除语义，后者会误删内容寻址共享行。
