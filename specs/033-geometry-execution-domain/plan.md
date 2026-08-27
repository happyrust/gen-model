# 033 几何执行域与可归因并发控制实施计划

## Constitution Check

- **I 水位是承诺**：本规格只改「几何活跑在哪个执行域、由谁分派」，不碰
  `applied_sesno`、尾事务、`gen_root` 完成凭证与持久补偿队列。CPU 段离闸之后，
  数据库读写仍在原来的 async 侧、原来的顺序上发生，暂存窗口（ADR-017）的提交
  边界与 `STAGED_COMMIT_SERIAL` 串行语义不变。
- **II 一条规则只有一份实现**：额度权威仍然只有 `src/fast_model/concurrency.rs`
  一处。`run_gated_cpu` 取代 `spawn_gated_leaf` 成为唯一过闸入口，
  `GeometryGate::acquire` 与 `GeometryPermit` 收成模块私有，调用方无法自建第二套
  口径。手动路径与 watcher 路径共用同一个执行域，不引入第二条数据批次消费路径
  （ADR-011 不动）。
- **III 静默失效零容忍**：执行域提交失败、CPU worker panic、动态领取队列异常一律
  上浮为该根的失败，不得吞成「少算一件」。新增观测项测不到就报 `None`，不许再出现
  `model_concurrency.rs:222` 那种恒 `0` 的 `shape_queue_depth`——一个永远是 0 的
  队列深度和没有这个字段是一回事，但它看起来像有。
- **IV 队列任务三条出路**：`model_update_pending` 的 attempts / revision / 死信裁决
  不动，CPU 段失败仍走既有的逐根收口路径，本规格不新增任何队列 action。
- **V 标识只用真值**：不涉及 `Ref0` / dbnum 解析。对应到本规格的形态是指标口径：
  `active` / `waiting` / `permit_wait_micros` 要到执行域切换之后才第一次是真值，
  改动前采到的读数只能作为「不可归因」的记录，不得回填进 A/B 证据。
- **VI 不变量由可执行的守护看住**：「许可只罩 CPU 段」的主要证明从源码字符串扫描
  换成类型——同步闭包里写不出 `.await`，「持许可等另一个 gated 子任务」在类型层
  不再可能。`concurrency.rs:236` 的扫描断言保留为补充，并按 ADR-052 补上现在漏网的
  形态（`use ... as`、子目录、`buffer_unordered`、`JoinSet`、直调
  `geometry_gate().acquire()`、`.chunks(20)` / `CHUNK_SIZE = 100`）。

**运行环境**：Windows / PowerShell / nightly，禁 `cargo clean`；live 一律用独立的
SurrealDB 2.1.x 数据目录，不碰已被 3.x 写坏的 `.surreal/ams-8009`。

## Complexity Tracking

两处偏离，都无法靠改设计消掉，按宪法「Development Workflow 第 3 条」在此写明。

1. **新增一个专用有界执行域**，与 `src/data_interface/model_concurrency.rs` 模块头
   「不创建新的线程池、Rayon 池或信号量」的既有措辞冲突。无法避免：CPU 段留在 tokio
   worker 上时，「限住 CPU」和「不饿死 runtime」这两个目标互斥——额度开大就挤掉
   shape receiver、SurrealDB response、watcher、timer 与 `/health`，额度收小就压住
   吞吐，而这正是当前 `permits = 8` 只兑现约 2.3 路的现场形态。缓解措施：执行域是
   进程内单例，线程数恒等于 `geometry_workers`，**不得**使用全局 Rayon 池，且由
   `geometry_executor` 开关整体关回旧路径。上述措辞随本规格落地一并修订，不留两份
   互相矛盾的文档。
2. **ISSUE-023 未闭合**：非 Windows 上 `acquire_process_instance_lock` 直接返回
   `Ok(())`，于是「进程内全局额度 = 全局额度」在 CentOS 7 上不成立（spec FR-1）。
   本规格不修它，但把它登记为**发布阻断项**：ISSUE-023 关闭之前，本规格的一切性能
   结论只在单实例的 Windows 开发机上成立，不得作为 CentOS 交付依据。

## 前置门与停止门

- **前置**：ISSUE-023 必须先有决断——要么实现 Unix `flock` 分支，要么在 issue 里
  写明现场靠什么（systemd / 容器 / 人工）保证单实例并登记发布阻断项。两者都没有，
  阶段 0 之后不得继续。
- **停止门**：同一组生成根、同一 `geometry_workers`、`model_concurrency_mode = "bounded"`
  且 K 固定时，连续三轮的模型阶段墙钟离散度必须 ≤ 10%。不可重复就先修可重复性，
  在此之前任何提速数字都是噪声，本计划停在阶段 0。

## 实施阶段

### 阶段 0：可归因基线（不改调度行为）

1. 处置 ISSUE-023（见前置门），结论写回 `issues/ISSUE-023-no-process-instance-lock-off-windows.md`。
2. 建 `docs/evidence/2026-08-27-geometry-execution-domain/baseline/`：二进制 / 源码 /
   config 哈希、数据快照、根集合、`geometry_workers` 依次取 1 / 2 / 4 / 8 / 物理核数，
   记录 spec「验证方法」列出的全部字段与派生量 `blocked_producer_equivalent`。
3. 补齐改动前测不到的三项观测：`concurrency.rs` 累计许可**持有**时长
   （现在只有 `WAIT_MICROS` 等待时长，没有持有时长，gate 利用率因此不可算）、
   tokio event-loop lag 采样、模型生成期间的 `/health` 分位数。只加计量，不改调度。
4. 用同一配置连跑三轮，执行停止门。

**阶段门**：三轮离散度 ≤ 10%，且 gate 利用率、event-loop lag 两条曲线拿得到数。

### 阶段 1：执行域地基

1. `src/fast_model/concurrency.rs`：新增 `run_gated_cpu(FnOnce() -> anyhow::Result<T>)`，
   专用有界池（独立 `ThreadPoolBuilder::num_threads(quota)` 或有界 `spawn_blocking`，
   **禁用全局 Rayon 池**），许可在 CPU worker 线程内取得与归还，不在等待
   `JoinHandle` 的 async 外壳上持有。
2. `GeometryGate::acquire` 与 `GeometryPermit` 由 `pub` 收成模块私有；
   `spawn_gated_leaf` 标记退役，保留一个发布周期。
3. `src/options.rs`：`geometry_executor = "tokio_legacy" | "blocking_pool"` 进
   `DbOptionExtFields`；`DbOption.toml` 与 `python/testbed/DbOption-pytest.toml`
   **同步加键**——根配置加必填键不同步，config 反序列化直接 missing field（AGENTS.md）。
4. `ACTIVE` 的语义从「有多少个 future 在飞」改成「有多少个 CPU 段在算」，
   `GeometryConcurrencySnapshot` 增加持有时长与利用率。

**阶段门**：`cargo check` 与 `concurrency` 全部单测绿；两个执行域开关档位的产物
逐哈希等价。

### 阶段 2：首站迁移 `manifold_bool.rs`

`manifold_bool.rs:78-83` 的目录布尔叶子现在是一张许可罩住
「SurrealDB 查 → `load_manifold` → 布尔 → mesh 转换 → `ser_to_file` → SurrealDB 写回」
全链。拆成：async 侧读 → `run_gated_cpu`（`load_manifold` + 布尔 + mesh 转换 + 同步
文件写） → async 侧写回。同步改写 `manifold_bool.rs:718` / `:741` 的源码形状断言，
否则这一步一动就红。

**阶段门**：布尔段 gate 利用率 ≥ 0.7（改动前不可测），结果逐哈希等价，
`permit_wait_micros` 第一次可归因。

### 阶段 3：其余叶子迁移与动态领取

1. 按收益排序迁移：`cata_model.rs:1934`（resolve 段是 258s 里最大的一块，排第一）、
   `prim_model.rs:71`、`loop_model.rs:46`、`gen_model.rs:851` / `:877`、
   `occ_generate.rs:645`。每文件一个可独立回滚的提交。
2. `occ_generate.rs` 若 `specs/009-retire-occ` 先行落地则直接删除、不迁——动手前
   先跟 009 对进度。
3. 静态均分改动态领取：`cata_model.rs:1915`、`prim_model.rs:62`、`loop_model.rs:37`、
   `manifold_bool.rs:78` 的 `chunk_size` fan-out 换成 `min(quota, job_count)` 个常驻
   worker 共享队列逐件领取。**不得**改成「每个元素一个 task 再去挤信号量」。
4. 逐文件核 ADR-052 的「未决」项：`occ_generate.rs` / `cata_model.rs` / `prim_model.rs` /
   `loop_model.rs` / `pdms_inst.rs` / `batch_worker.rs` 里是否还有未过闸的 fan-out
   或阻塞调用，核完写进证据目录。

**阶段门**：CATA 段 p50 / p95 相对基线下降 ≥ 50%（对齐 specs/032 的硬门）；
CPU / wall 从约 2.3 抬到 ≥ 5（`permits = 8`）；结果逐哈希等价。

### 阶段 4：额度回到单一职责，shape 观测说真话

1. **SQL 攒批解耦**：`manifold_bool.rs:368` 与 `cata_model.rs:880` 的
   `geometry_gate().chunk_size()` 改由独立的行数 / 字节预算决定
   （`shape_save` 已有 `sql_bytes` 口径可拄）。回归钉：`geometry_workers = 1` 时
   SQL 包不再放大到整个输入。
2. **背压指标按 channel 统计**：`shape_save.rs:65-75` 的 `PRODUCER_BLOCKED_NANOS`
   现在是进程全局累加，区分不出快路径与真阻塞。改成按 channel 独立统计，
   分开 `try_send` 快路径与 `Full` 之后的真实阻塞时长，并公开 send 次数、full 次数、
   队列高水位、单 batch 最大字节、writer busy time；`model_concurrency.rs:222` 的
   `shape_queue_depth: 0` 换成真值。
3. **字节估算不再造一份临时 JSON**：`shape_save.rs:123` 的 `serde_json::to_vec(batch)`
   改为按行数 / 几何 occurrence / 字符串长度增量估算，保留「宁可估大、提前 flush」
   的保守方向。`gen_model.rs:244` / `:313` 的 `flume::bounded(CHUNK_SIZE)` 从纯条数
   预算升级为条数 + 字节双限。

**阶段门**：固定负载下 `sql_packets` 与 `sql_bytes` 在不同 `geometry_workers` 下一致；
`shape_queue_depth` 不再恒 0；写 p95 不高于基线 1.25 倍。

### 阶段 5：控制器归位与正式 A/B

1. `model_concurrency_mode` 在本规格内固定为 `bounded`：`DbOption.toml:254` 与
   `python/testbed/DbOption-pytest.toml:134` 现在都是 `adaptive`，连同
   `options.rs:461` 的默认回落一起改，并同步 `options.rs:897` 那条断言默认是
   `adaptive` 的测试——默认值是行为，改默认必须有一条会红的测试跟着动。
2. `gen_model.rs:358-368`：`shape_pressure` 与 `aabb_pressure` 不再进
   `record_window` 的 `pressured`（spec FR-9）。shape 背压只能调节 instance 生产准入、
   writer 字节预算，或作为健康告警——它本来就不控制根在飞数，用它去调是传感器与
   执行器错位。
3. adaptive 重新启用的六个条件（K 显式传入结算、按 execution group 取增量、
   K=1 与 K=2 样本互不污染、基线用分位数滑窗而非 `fetch_max`、确定性几何失败不计入
   资源压力、升档需要正向条件）写进 `model_concurrency.rs` 模块文档，作为下一个规格
   的入口。**本规格内不启用**。
4. 正式 A/B：8000 实机（现状约 258s）与空库全量（现状约 808s）两套各三轮，随机顺序，
   以 `model_ready = true` 最终收敛为停止边界。

**阶段门**：spec「成功标准」全部通过；峰值 RSS ≤ 基线 1.25 倍；关闭全部新开关后
行为与指标回到基线路径。

## 配置与即时回滚

```toml
geometry_executor = "tokio_legacy"   # tokio_legacy | blocking_pool
geometry_fanout = "static_chunk"     # static_chunk | dynamic_claim
geometry_sql_batch_rows = 300
geometry_sql_batch_bytes = 1048576
shape_channel_max_batches = 100
shape_channel_max_bytes = 16777216
model_concurrency_mode = "bounded"   # legacy | bounded | adaptive
```

回滚顺序：`geometry_fanout` 回 `static_chunk` → `geometry_executor` 回 `tokio_legacy`
→ SQL 预算回派生宽度 → shape channel 回纯条数预算。全部是进程内执行域与预算切换，
不涉及数据库 schema、水位或 pending 修复。**`geometry_workers = 1` 不再是回滚手段**
（ADR-052 决策 6）：它只能把过闸的叶子串行化，管不到 writer、数据库 I/O、编排任务
与未过闸的 CPU，还会把顺序 SQL 批放大到整个输入。

## 质量门与交付物

- `cargo fmt`、`cargo check`
- `cargo test --locked --lib concurrency --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture`
- 同口径再跑 `--lib shape_save`、`--lib model_concurrency`、`--lib options`
- CI 的四个集成测试目标：`db8000_two_delete_fixture`、`db_session_fixture_selfcheck`、
  `db8000_session_pairs`、`pdms_record_boundary`
- CI Release 构建命令（`--no-default-features --features ws,gen_model,manifold,project_hd,http_api`）
- `powershell -File scripts\Test-DbOptionDrift.ps1 -Mode Staged`——本规格新增五个配置键，
  这一条不是可选项
- `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`
- 证据落 `docs/evidence/2026-08-27-geometry-execution-domain/`；动过 live 用例
  同步 `docs/2026-08-12_live-test-ledger.md`

## 与 specs/032 的边界与排他

032 改的是「做什么活」（CATA 产品构建、Shape packet 合并、AABB 查旧条目），
033 改的是「这些活跑在哪个执行域、由谁分派」。两者都要动
`src/fast_model/cata_model.rs` 与 `src/fast_model/gen_model.rs`，约定：032 的
`CataProduct` 拆分（032 的 T006）先落地，033 阶段 3 的 `cata_model.rs:1934` 迁移在
其后接手；同一时间只允许一条线改这两个文件，另一条线等对方合入再 rebase。

## 决策与来源

- ADR-052（本计划的决策来源，六条决策逐条对应阶段 1～5）
- ADR-041 第 3 条 / `specs/023`：统一几何闸，本规格是它的修正而非推翻
- ADR-011 / ADR-017：单消费路径与暂存窗口串行语义，本规格不动
- `issues/ISSUE-023`：前置条件兼发布阻断项
- 现场数据来自 `test-worklspace` 2026-08-25 22:56 那轮（`watch_dbnums = [8000]`）
  与 `gen-model-cata-throughput` 的空库全量基准
