# 033 几何执行域与可归因并发控制任务

## P0：前置、基线与可归因

- [ ] T001 `issues/ISSUE-023-no-process-instance-lock-off-windows.md`：确认现场 CentOS
      靠什么保证单实例，要么实现 Unix `flock` 分支并把守卫测试跨平台化，要么在 issue
      里登记发布阻断项。两者都没有则本规格停在 P0。依赖：无。
- [x] T002 [P] `src/fast_model/concurrency.rs`：新增许可**持有**时长累计
      （原有 `WAIT_MICROS` 只记等待），`GeometryConcurrencySnapshot` 增
      `active_permit_micros` / `observed_micros` 与区间利用率
      `utilization_since`。只加计量，不改调度。依赖：无。
      2026-08-27 完成：计量从模块级 static 收进闸实例（临时闸各记各的，读数不再被
      同一二进制里并行跑的用例污染）；不提供「进程至今」的平均利用率，只给区间值；
      三条新测试（利用率纯函数不夹上界 / 快照做差 / 等待不计进持有），
      `--lib fast_model::concurrency` 九条全绿。
- [x] T003 [P] `src/runtime_lag.rs`（新建）、`src/lib.rs`、`src/web_service/handlers.rs`：
      加 tokio 调度延迟采样（定时器超时误差）并接进 `/health`，用于量化 CPU 段饿死
      调度的程度。依赖：无。
      2026-08-27 完成：采样任务在 `run_cli` 定死几何额度之后立即起，`run_app` 转手
      调 `run_cli` 故只起一次（`OnceLock` 幂等）；每轮独立计时、不补追落后轮次，
      一次 3 秒卡顿留一个 3 秒样本而不是三十个把 p50 冲淡的小样本；512 样本滚动窗口
      之外单独保留进程期最坏值；`sampling: false` 与「延迟为 0」在 `/health` 上分得开。
      分位数复用 `model_concurrency::percentile`，两个区块的 p95 是同一把尺子。
      三条新测试全绿。
- [ ] T004 `docs/evidence/2026-08-27-geometry-execution-domain/baseline/`：`geometry_workers`
      依次取 1 / 2 / 4 / 8 / 物理核数，记录 spec「验证方法」列出的全部字段、派生量
      `blocked_producer_equivalent`、CPU / RSS 与二进制/源码/config 哈希。依赖：T002、T003。
- [ ] T005 `docs/evidence/2026-08-27-geometry-execution-domain/repeatability/`：同一配置
      连跑三轮，执行 10% 离散度停止门；不达标先修可重复性，不进 P1。依赖：T004。

## P1：执行域地基

- [ ] T006 `src/fast_model/concurrency.rs`：新增 `run_gated_cpu(FnOnce() -> anyhow::Result<T>)`，
      专用有界池（`num_threads(quota)` 的私有 Rayon 池或有界 `spawn_blocking`，禁全局
      Rayon 池），许可在 CPU worker 线程内取得与归还。依赖：T005。
- [ ] T007 `src/fast_model/concurrency.rs`：`GeometryGate::acquire` 与 `GeometryPermit`
      由 `pub` 收成模块私有，`spawn_gated_leaf` 标记退役并保留一个发布周期；
      `ACTIVE` 语义改为「正在算的 CPU 段数」。依赖：T006。
- [ ] T008 `src/options.rs`、`DbOption.toml`、`python/testbed/DbOption-pytest.toml`：
      `geometry_executor = "tokio_legacy" | "blocking_pool"` 进 `DbOptionExtFields`，
      非法值启动失败不静默回退；两个配置文件同批加键。依赖：T006。
- [ ] T009 [P] `src/fast_model/concurrency.rs` 测试：额度 1 真串行、额度 0 夹到 1、
      执行域两档产物一致、闭包内无法 `.await`（类型层，编译期证明写成 doc test 或
      `compile_fail`）。依赖：T007。
- [ ] T010 `powershell -File scripts\Test-DbOptionDrift.ps1 -Mode Staged` 与
      `cargo test --lib options`：新配置键在两个 toml 与 `DbOptionExtFields` 三处一致。
      依赖：T008。

## P2：首站迁移与可归因验收

- [ ] T011 `src/fast_model/manifold_bool.rs`（叶子在 `:78-83`）：一张许可罩全链拆成
      async 侧读 → `run_gated_cpu`（`load_manifold` + 布尔 + mesh 转换 + `ser_to_file`）
      → async 侧写回。依赖：T009、T010。
- [ ] T012 `src/fast_model/manifold_bool.rs:718`、`:741`：源码形状断言随新入口改写，
      旧断言不改这一步就会红。依赖：T011。
- [ ] T013 `docs/evidence/2026-08-27-geometry-execution-domain/manifold-bool/`：布尔段
      gate 利用率 ≥ 0.7，结果逐哈希等价（复用 `gen-model-cata-throughput` 的
      export-db-equivalence 口径）。依赖：T012。

## P3：其余叶子、动态领取与纪律

- [ ] T014 `src/fast_model/cata_model.rs:1934`：resolve 段迁移，收益最大排第一；
      与 specs/032 的 `CataProduct` 拆分不得同时在飞。依赖：T013。
- [ ] T015 [P] `src/fast_model/prim_model.rs:71`：叶子迁移，独立提交。依赖：T013。
- [ ] T016 [P] `src/fast_model/loop_model.rs:46`：叶子迁移，独立提交。依赖：T013。
- [ ] T017 [P] `src/fast_model/gen_model.rs:851`、`:877`：两处叶子迁移。依赖：T013。
- [ ] T018 `src/fast_model/occ_generate.rs:645`、`:1475`：先与 `specs/009-retire-occ`
      对进度——OCC 退役先落地就直接删除不迁；否则迁移并同步源码断言。依赖：T013。
- [ ] T019 `src/fast_model/cata_model.rs:1915`、`prim_model.rs:62`、`loop_model.rs:37`、
      `manifold_bool.rs:78`：静态均分 `chunk_size` 改 `min(quota, jobs)` 个常驻 worker
      共享队列逐件领取；禁「每元素一 task 挤信号量」。依赖：T014、T015、T016、T017。
- [ ] T020 [P] `src/fast_model/concurrency.rs:236` 的 `no_hardcoded_fanout_width_survives_in_fast_model`：
      扩面到子目录，禁旧名 `spawn_gated_leaf`、禁直调 `geometry_gate().acquire()`，
      纳入 ADR-052 点名的漏网形态（`.chunks(20)`、`CHUNK_SIZE = 100`、
      `buffer_unordered`、`JoinSet`、`use ... as`）。依赖：T019。
- [ ] T021 `docs/evidence/2026-08-27-geometry-execution-domain/fanout/`：逐文件核
      `occ_generate.rs` / `cata_model.rs` / `prim_model.rs` / `loop_model.rs` /
      `pdms_inst.rs` / `batch_worker.rs` 有无未过闸的 fan-out 或阻塞调用（ADR-052
      「未决」项）；CATA p50/p95 −50%、CPU/wall ≥ 5。依赖：T019、T020。

## P4：额度单一职责与 shape 观测

- [ ] T022 `src/fast_model/manifold_bool.rs:368`、`cata_model.rs:880`：SQL 攒批宽度改
      独立行数 / 字节预算，不再从 `geometry_gate().chunk_size()` 派生。依赖：T011。
- [ ] T023 [P] 回归测试：`geometry_workers = 1` 时 SQL 包不再放大到整个输入，
      固定负载下 `sql_packets` / `sql_bytes` 在不同额度下一致。依赖：T022。
- [ ] T024 `src/fast_model/shape_save.rs:65-75`、`src/data_interface/model_concurrency.rs:222`：
      背压指标按 channel 独立统计，分开 `try_send` 快路径与 `Full` 后真实阻塞，
      公开 send / full 次数、队列高水位、单 batch 最大字节、writer busy time；
      `shape_queue_depth` 报真值不再恒 0。依赖：T013。
- [ ] T025 `src/fast_model/shape_save.rs:123`、`gen_model.rs:244`、`:313`：
      `BatchMeasure` 的字节估算不再 `serde_json::to_vec` 造临时 JSON，改增量估算且
      保持保守偏大；`flume::bounded(CHUNK_SIZE)` 升级为条数 + 字节双限。依赖：T024。
- [ ] T026 [P] `src/fast_model/shape_save.rs` 测试：估算值单调、不低于真实序列化长度的
      安全下界、双限任一触发即 flush、`FlushReason` 分类不变。依赖：T025。

## P5：控制器归位、正式 A/B 与交付

- [ ] T027 `src/options.rs:461`、`:897`、`DbOption.toml:254`、
      `python/testbed/DbOption-pytest.toml:134`：`model_concurrency_mode` 固定为
      `bounded`，默认回落与那条断言默认是 `adaptive` 的测试同批改。依赖：T021。
- [ ] T028 `src/fast_model/gen_model.rs:358-368`：`shape_pressure` 与 `aabb_pressure`
      退出 `record_window` 的 `pressured`；shape 背压改接 instance 生产准入 / writer
      字节预算 / 健康告警。依赖：T027。
- [ ] T029 [P] `src/data_interface/model_concurrency.rs` 模块文档：写下 adaptive 重新
      启用的六个条件（K 显式传入、按组取增量、K 样本不互污、分位数滑窗替
      `fetch_max`、确定性几何失败不计压力、升档需正向条件），本规格内不启用。
      依赖：T028。
- [ ] T030 `docs/evidence/2026-08-27-geometry-execution-domain/ab/`：8000 实机与空库
      全量两套各三轮随机顺序，以 `model_ready = true` 收敛为停止边界；核 spec
      「成功标准」全部条目。依赖：T021、T023、T026、T029。
- [ ] T031 跑 `cargo fmt`、`cargo check`、`--lib concurrency` / `shape_save` /
      `model_concurrency` / `options`、CI 四个集成测试、CI Release 构建；
      命令 / 字面输出 / 退出码全部入验证记录。依赖：T030。
- [ ] T032 [P] 更新 `changelog.md`、`docs/adr/ADR-052-geometry-quota-covers-cpu-only.md`
      实施注记、`docs/2026-08-12_live-test-ledger.md`、
      `src/data_interface/model_concurrency.rs` 模块头「不创建新线程池」的措辞
      （见 plan 的 Complexity Tracking 第 1 条）与本文件任务状态。依赖：T030。
- [ ] T033 执行 `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`，
      并核对没有第二条模型消费路径、第二个 Shape writer、并行 AABB 发布或第二套
      并发口径。依赖：T031、T032。

## 依赖顺序与并行说明

- 主链：T001→T004→T005→T006→T008→T011→T013→T014→T019→T021→T027→T030→T031→T033。
- T022～T023（SQL 攒批解耦）在 T011 之后即可与 P3 并行，不必等动态领取。
- T024～T026（shape 观测与字节估算）在 T013 之后与 P3 并行，文件所有权不交叉。
- `[P]` 只表示文件所有权无交叉；共享 `cata_model.rs`、`gen_model.rs` 或同一份运行
  配置时仍然串行。
- 与 `specs/032` 排他：`cata_model.rs` 与 `gen_model.rs` 同一时间只允许一条线在改，
  032 的 `CataProduct` 拆分先落地，T014 在其后接手。

## 完成定义

T001～T033 全部完成、spec「成功标准」逐条通过、生成语义逐字段哈希一致、关闭全部新
开关能回到基线路径、范围外零变化，本规格才可标记完成。单点利用率读数、局部测试通过
或某一轮墙钟变快都不构成完成证据；ISSUE-023 未闭合时，结论只在 Windows 单实例下成立。
