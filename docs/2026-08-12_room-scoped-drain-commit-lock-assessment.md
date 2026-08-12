# 评估：把提交后 scoped room drain 挪出 `STAGED_COMMIT_SERIAL` 的可行性与所需闸门

日期：2026-08-12
状态：评估结论（**现在不动**；瓶颈出现时按 §5 的杠杆顺序处置）
关联：ADR-017 §5 与结果/约束（2026-08-12 补记）；ADR-010（2026-08-09 修订）；
ADR-011（2026-08-09 修订：并发派发 + 提交串行）；
`src/data_interface/batch_worker.rs`（`STAGED_COMMIT_SERIAL`、`execute_staged_batch`、
派发门）；`src/data_interface/model_update_pending.rs`（`drain_rooms_scoped`）

## 1. 问题

本任务 scoped room drain 与 journal 写回、水位尾事务、空间收敛同处
`STAGED_COMMIT_SERIAL` 临界段（`execute_staged_batch` 拿锁后持到函数返回）。
房间重算要读 mesh 做逐点包含判定，其时长直接顶住下一窗口的写回与派发门。
ADR-010 第 1 条「不进水位事务」当年挡掉的秒级拖累，在锁维度回来了一半：
容器搬迁牵出上百分支重生成时，本任务元素目标可上千，临界段随之分钟级拉长。

ADR-011 2026-08-09 修订对串行段的论证是「两个窗口的全局 drain / 空间树收敛
交错在正确性上没有论证过，而串行的代价（**秒级**）远小于生成（分钟级）」——
房间时长恰恰可以不是秒级，所以值得单独评估。

## 2. 锁内执行当前买到的三个不变量

1. **空间树新鲜且中途不动。** scoped drain 紧跟在本窗口的
   `reconcile_spatial_pending` 之后；派发门的空间收敛与其他窗口的提交都持同一把
   锁，drain 期间没人动树。整间分支的成员候选取自 `GLOBAL_AABB_TREE`，树中途
   变层 = 同一轮内不同面板用不同基线。
2. **持久层无并发半写。** 房间重算是**非水位读者**（`PanelIndex`、
   `ElementRoomHistory`、实例包围盒读的都是活行）。锁挡住了其他窗口的 journal
   重放，drain 读到的恒为「已收口的窗口终态」。ADR-017 phase-1 自觉保留的
   「写回一半时非水位读者可见秒级窗口」这条残余，正是靠锁没有伤到房间。
3. **与空闲房间轮天然互斥。** 今天由 worker 时序保证：空闲轮只在队列跑空、
   批次收尾之后运行，scoped drain 只在批次执行中运行，两个 drain 不会对同一
   target 交错做「先清后写」。

## 3. 挪出锁所需的等价闸门

- **G1 新鲜度闸**：开始前 `has_pending_spatial_work == false`（room_round 已有
  同款）；且对「drain 进行中别人又提交了」要有感知——每页开始时比对
  `spatial_epoch`（尾事务本来就 bump），变了就把余下目标留 durable pending
  退出本轮，交空闲轮。缺这道闸 = 拿陈旧树改写归属，正是 room_round 那道闸
  防的事故。
- **G2 半写隔离闸**：drain 的读集（scope 目标 + 其候选面板的几何/历史边）不得
  与任何在飞窗口的写回写集相交。粗粒度实现（drain 期间禁止新窗口进入写回）
  等于把锁换个名字；细粒度实现要拿计划层的 regen 根子树/refresh 集与 scope
  求交，成本不小且引入新的失效面；根治是 ADR-017 §10 的 phase-2 暂存会话
  （overlay 快照读，引擎吸收本闸）。
- **G3 单飞闸**：显式的 room drain 互斥（scoped 与空闲轮共用一把），取代今天的
  worker 时序保证。两个 drain 双写者对同一 target 的 DELETE→RELATE 交错虽因
  每目标事务化而最终收敛，但吸收封闭性判定会在混合状态上做出不同裁决，白跑
  之外还制造难复现的日志。

## 4. 收益边界

- 默认 `data_batch_workers = 1` 时，worker 本就串行消费，把 drain 挪出锁**不会
  让任何批次提前提交**；唯一被放开的是派发门的空间收敛与房间并发——那恰是
  G1 要防的事。净收益 ≈ 0，纯背风险。
- `data_batch_workers > 1` 时收益真实存在（房间时长不再挡下一窗口写回），但
  G2 的粗粒度实现会把收益吃回去，细粒度实现的复杂度与 phase-2 的收益重叠。

## 5. 结论与杠杆顺序

**现在不动。** 临界段时长真成为瓶颈时，按序取杠杆：

1. **inline 预算**：给 scoped drain 加时间/条数预算，超预算的余量目标原样留
   durable pending 交空闲房间轮（语义现成——「失败留 pending、任务记
   partial」的通道就是它，预算只是把「失败」换成「主动让路」）。改动最小，
   临界段时长立即有上界。
2. **drain 内并行化**：mesh 逐点判定按页内并行（目标间无共享可变状态），
   不改任何边界与锁。
3. **phase-2 暂存会话落地后**再考虑真正的锁外 drain：G2 由引擎快照吸收，
   只剩 G1/G3 两道闸，方案才配得上复杂度。

观测已就位：批次日志「写回后房间计算 … room_duration_ms=…」与
「数据批次 阶段耗时 … 房间={room_ms}ms」两行，够判断瓶颈是否出现。
