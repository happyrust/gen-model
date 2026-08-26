# ADR-052：几何并发额度只覆盖 CPU 执行段

- 状态：Proposed
- 日期：2026-08-26
- 关联：ADR-011（唯一批次队列与单协调器）、ADR-017（暂存窗口提交）、ADR-041（第 3 条：统一几何并发口径）、ADR-050（进程级模型工作单）、`specs/023-parallel-root-generation-pipeline`（几何闸落地）、`specs/032-model-generation-throughput-closure`（CATA / Shape / AABB 吞吐）

## 背景

ADR-041 第 3 条与 `specs/023` 把散落各处的写死 fan-out 收成一个全局信号量
`GeometryGate`，消灭了「8 个根 × 16 路 = 128 个 task 同抢 CPU」那个算不清的乘积。
额度成为唯一的性能旋钮兼回滚开关。这一步是对的，但它把额度装在了错误的位置上。

四条现状，都有源码：

1. **许可覆盖整个 future，而不是 CPU 段。** `spawn_gated_leaf` 是
   `let _permit = gate.acquire().await; future.await`。`manifold_bool.rs` 的
   目录布尔叶子在一张许可之内依次做了：SurrealDB 查询 → 同步 `load_manifold` →
   manifold 布尔 → mesh 转换 → 同步 `ser_to_file` → SurrealDB 写回。
2. **暂存写在一把 mutex 后面串行，且锁跨 `.await` 持有**
   （`staging/write_context.rs` 的 `self.executor.lock().await.execute(sql, mode).await`）。
   于是 16 张许可可以全部被「等同一把暂存锁」的任务占住，`ACTIVE == 16` 而实际
   只有一个任务在用 CPU；准备好做几何的任务反而进不了闸。这不是死锁，是**容量倒置**，
   典型的 convoy。
3. **同一个额度还兼任顺序 SQL 攒批宽度。** `manifold_bool.rs` 用
   `geometry_gate().chunk_size(boolean_query.len())` 决定一批 `update_sql` 装多少元素。
   于是调 CPU 并发会同时改变 SQL 包大小、写 p95、失败粒度与内存占用；
   `geometry_workers = 1` 时 CPU 串行，SQL 批却可能扩到整个输入。
4. **分块是静态均分。** `chunk_size = len.div_ceil(quota)` 只保证块数不超过额度，
   不保证接近额度：`quota = 16`、`len = 17` 时只切出 9 个任务，`len = 33` 时 11 个。
   而几何负载天然长尾，静态分区一旦把几个重 BRAN 分进同一块，其余 worker 早已空转。

另有一条纪律层面的问题：「许可只准叶子持有」目前靠 `src/fast_model` 第一层的源码字符串
扫描维持。它能挡住 `Semaphore::new(` 和 `.len() / 16` 这类旧写法，挡不住 `use ... as S`、
`items.chunks(width)`、子目录文件、`buffer_unordered`、`JoinSet`、直接调用公开的
`geometry_gate().acquire()`，也挡不住「父 gated 任务 await 子 gated 任务」这一条真正
会死锁的写法。仓内现存的 `.chunks(20)` / `.chunks(200)` / `CHUNK_SIZE = 100` 就在
扫描范围之外。

## 决策

1. **额度只覆盖同步 CPU 段。** 退役接受任意 `Future` 的 `spawn_gated_leaf`，
   改为只接受同步闭包的 `run_gated_cpu(FnOnce() -> Result<T>)`。闭包里写不出 `.await`，
   「持许可等待另一个 gated 子任务」在类型层面不再可能。
2. **`GeometryGate::acquire` 与 `GeometryPermit` 收成模块私有。** 调用方无法自行扩大
   许可作用域。源码扫描断言保留为补充，但不再被当作并发正确性的主要证明。
3. **CPU 段离开 tokio worker。** 纯几何计算与同步 mesh 文件写入送进专用有界执行域
   （`spawn_blocking` 有界池或专用 `ThreadPoolBuilder::num_threads(geometry_workers)`
   的 Rayon 池，**不得用全局 Rayon 池**）。许可在 CPU worker 内持有，不在等待
   `JoinHandle` 的 async 外壳上持有。数据库读写留在 async 侧，不占许可。
4. **fan-out 改为动态领取。** `min(quota, job_count)` 个常驻 worker 从共享队列逐件
   （或按 micro-batch）领取，取代静态均分。不得改成「每个元素一个 task 再去挤信号量」。
5. **SQL 攒批宽度与 CPU 额度解耦。** 顺序循环里的写回批大小改由独立的行数 / 字节数
   预算决定，不再从 `geometry_gate().chunk_size()` 派生。
6. **回滚不靠 `geometry_workers = 1`。** 它只能把过闸的叶子串行化，管不到 writer、
   数据库 I/O、编排任务与未过闸的 CPU，且会把顺序 SQL 批放大。执行域切换必须有
   自己的配置开关（`geometry_executor = "tokio_legacy" | "blocking_pool"`），
   旧路径保留一个发布周期。

## 结果

- 额度语义从「同时有多少个 future 在飞」变成「同时有多少个 CPU 段在算」，
  `active` / `waiting` / `permit_wait_micros` 第一次可归因。
- 等数据库、等暂存锁的任务不再占用 CPU 额度，`model_root_inflight_max` 的
  latency hiding 才有可能兑现。
- `geometry_workers` 回到单一职责；SQL 包大小、写 p95、失败粒度不再随它漂移。
- CPU 密集段不再占住 tokio worker，shape receiver、SurrealDB response、watcher、
  `/health` 与 timer 不再被三角化和布尔运算挤掉调度。
- 代价：多一层执行域切换与一次跨线程数据搬运；需要新的开关与一轮 A/B。

## 未决

- `occ_generate.rs` / `cata_model.rs` / `prim_model.rs` / `loop_model.rs` /
  `pdms_inst.rs` 是否还有未过闸的 fan-out 或阻塞调用，本决策成文时未逐一核过。
  迁移按 `manifold_bool.rs` → 其余的顺序推进，每个文件一个可独立回滚的提交。
- 单进程假设本身有缺口：非 Windows 上 `acquire_process_instance_lock` 直接返回
  `Ok(())`，所谓「全局」额度只是进程内全局。见 `issues/ISSUE-023`。
  该缺口不修，本决策在 CentOS 7 上不成立。
