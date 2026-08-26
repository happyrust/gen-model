# 033 几何执行域与可归因并发控制规格

## 背景

`specs/023` 把几何 fan-out 收进一个全局信号量后，额度成了唯一的性能旋钮。但额度装在
`spawn_gated_leaf` 的整个 `Future` 上，而这个 future 里既有 CPU 布尔和三角化，也有
SurrealDB 查询、跨 `.await` 持有的暂存 mutex 和同步文件写。结果是 CPU 额度被 I/O
等待者占据（容量倒置），`active` 计数与真实 CPU 占用脱钩，任何基于它的调参都无法归因。

同一个额度还兼任顺序 SQL 攒批宽度，因此 `geometry_workers` 一动，CPU 并发、SQL 包大小、
写 p95、失败粒度和内存同时变。

在这之上，`model_concurrency` 的自适应控制器有三处传感器 / 执行器错位：它在
execution group 结束后才结算（30 秒只是最短调节间隔，不是采样周期）；它把 shape
producer 背压算进压力去调根在飞数，而根在飞数根本不控制 instance 生产阶段；它的
K=1 基线用 `fetch_max` 取进程期高水位，一次偶发慢写可以永久抬高基线。

`specs/032` 处理的是 CATA 产品构建、Shape packet 与 AABB 查旧条目；本规格处理的是
它们脚下的执行域与控制回路，两者不重叠。

## 功能要求

1. **前置**：非 Windows 上必须有真实的进程单实例锁（`issues/ISSUE-023`）。该缺口未闭合前，
   本规格的一切「全局额度」结论在 CentOS 7 上不成立，性能实验也不得作为发布依据。
2. 几何并发许可只覆盖同步 CPU 段。对外只暴露接受同步闭包的入口；
   `GeometryGate::acquire` 与许可类型对模块外不可见。
3. 几何 CPU 段与同步 mesh 文件写入在专用有界执行域内运行，不占用 tokio runtime 的
   通用 worker。数据库读写留在 async 侧且不占许可。
4. fan-out 采用动态领取（`min(quota, job_count)` 个 worker 从共享队列取件），
   取代 `chunk_size = len.div_ceil(quota)` 的静态均分。并行度在工作量超过额度时
   不得低于额度。
5. 顺序循环里的 SQL 攒批宽度由独立的行数 / 字节数预算决定，与 `geometry_workers` 解耦。
6. Shape 背压指标按 channel 独立统计，不再用进程全局累加：区分 `try_send` 快路径与
   `Full` 后的真实阻塞时长，并公开 send 次数、full 次数、队列高水位、单 batch 最大字节、
   writer busy time。`shape_queue_depth` 不得固定为 0。
7. Shape batch 的字节估算不得为此完整生成一份临时 JSON；channel 预算从纯条数升级为
   条数 + 字节双限。
8. 自适应控制器在本规格内先固定为 `bounded`。重新启用自适应前必须满足：
   本轮实际使用的 K 显式传入结算、按 execution group 取指标增量而非全局瞬时值、
   K=1 与 K=2 的样本互不污染、基线用分位数滑窗而非 `fetch_max`、确定性几何失败
   （NotManifold、缺数据、坏几何）不计入资源压力、升档需要正向条件（有根积压且
   gate 利用率不足）。
9. shape 背压不得再直接调节根在飞数；它只能调节 instance 生产准入、writer 字节预算，
   或作为健康告警。
10. 每一项改动有独立配置开关，关闭后回到当前已验证路径，不要求数据库 schema、
    水位或 pending 修复。`geometry_workers = 1` 不得作为唯一回滚手段。
11. 生成语义不变：`inst_info`、`inst_relate`、`inst_geo`、`geo_relate`、正负 / 布尔关系、
    规范化 mesh、AABB、空间关系、房间关系、`gen_root` 完成凭证与水位逐字段语义哈希一致。

## 非目标

- 不增加 Shape writer。暂存模式下所有写最终经过同一把 `StagedExecutor` mutex，
  多 writer 只会增加计划构建、锁竞争与内存中间态。
- 不改几何算法、mesh 容差、布尔算法、生成根枚举与房间归属规则。
- 不在本规格内提高 `model_root_inflight_max` 的上限。
- 不重做 `specs/032` 已覆盖的 CATA 产品构建、Shape packet 合并与 AABB 查旧条目。
- 不引入第二条数据批次消费路径（ADR-011）。

## 成功标准

- 同一组生成根、同一 `geometry_workers`、`model_concurrency_mode = "bounded"` 且 K 固定时，
  连续三轮的模型阶段墙钟时间离散度不超过 10%——先要可重复，才谈得上提速。
- gate 利用率（`active_permit_micros / (quota × wall)`）在 CPU 密集阶段不低于 0.7；
  改动前该值不可测，属于本规格新增的观测项。
- tokio event-loop lag 的 p99 相对基线下降至少一个数量级；`/health` 在模型生成期间
  的 p99 不高于空闲期的 2 倍。
- 模型阶段墙钟时间不劣于基线；`geometry_workers` 从 1 扫到物理核数时
  roots/s 单调不降（当前静态分区下不成立）。
- Surreal 写 p95 不高于基线 1.25 倍，峰值 RSS 不超过基线 1.25 倍。
- `geometry_workers` 变化不再改变 SQL 包大小：固定负载下 `sql_packets` 与 `sql_bytes`
  在不同额度下一致。
- 关闭所有新开关后，行为与指标回到基线路径。

## 验证方法

受控 A/B，每轮固定同一组生成根，`model_concurrency_mode = "bounded"`、K 固定，
`geometry_workers` 依次取 1 / 2 / 4 / 8 / 物理核数，记录：
`instances`、`source_batches`、`flushes`、`sql_packets`、`sql_bytes`、
`producer_blocked_ms`、instance 阶段墙钟 `T_instance`、`permit_wait_micros` 增量、
Surreal 读写 p50 / p95 / p99 与 retry、进程 CPU 与 RSS。

派生量 `blocked_producer_equivalent = producer_blocked_ms / T_instance_ms`
读作「平均有多少个 producer 在等」，不是百分比。判读口径：

- `instances/s` 不再随额度增长而 `blocked_producer_equivalent` 上升 → writer 或数据库
  服务率是瓶颈；
- blocked 低、CPU 高、吞吐仍随额度增长 → writer 不是当前瓶颈；
- blocked、写 p95、retry 同时上升 → 更像数据库 / 暂存执行器瓶颈；
- 加大 channel 只降 blocked 而总墙钟不变 → 只是加了缓冲，服务率没变。

## 决策引用

- ADR-052（本规格的决策来源）：几何并发额度只覆盖 CPU 执行段。
- ADR-011：单协调器与单模型消费路径。
- ADR-017：暂存窗口提交，`StagedExecutor` 的串行语义是本规格的既定约束。
- ADR-041 第 3 条 / `specs/023`：统一几何闸与有界根级并发，本规格是它的修正而非推翻。
- `specs/032-model-generation-throughput-closure`：CATA / Shape packet / AABB 吞吐，
  与本规格并行推进，边界见「非目标」。
- `issues/ISSUE-023`：非 Windows 无进程单实例锁，本规格的前置条件。

## 来源与证据边界

本规格的问题清单来自 2026-08-26 的一次外部模型审核（oracle，GPT-5.6 Sol，Pro thinking，
会话 `model-gen-concurrenc-efficiency-review`），送审文件 13 个、约 114k tokens。
以下结论**已在本仓源码上逐条复核**：非 Windows 空实现的单实例锁、许可覆盖整个 future、
暂存 mutex 跨 `.await`、`geometry_gate().chunk_size()` 兼任 SQL 攒批宽度、
`record_window` 结束时重读全局 K、`shape_pressure` / `aabb_pressure` /
`geometry.waiting > quota × 2` 三者一并计入压力、`shape_queue_depth` 固定为 0、
`BatchMeasure` 用 `serde_json::to_vec` 估算字节。

以下仍是**待数据确认的推断**，不得写进任务验收：单 receiver 是否是当前第一瓶颈、
动态调度相对静态分区的实际收益幅度（取决于重任务的数量与在输入中的聚集程度，
若单个最大 BRAN 就占总 CPU 的大头则收益有限）、tokio 调度器当前被饿死到什么程度、
64MB 线程栈在 CentOS 上的真实内存代价。

审核当时未送进上下文、因而其内部并发形态未经核实的文件：`occ_generate.rs`、
`cata_model.rs`、`prim_model.rs`、`loop_model.rs`、`pdms_inst.rs`、`batch_worker.rs`。
