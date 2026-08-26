# 023 并行生成与收口提速任务

- [ ] T01 在 `docs/evidence/2026-08-21-parallel-root-generation/baseline/` 记录改动前代码哈希、
      配置与运行环境。
- [ ] T02 给全量基线加分段计时（解析 / 生成 / 写回 / 空间树 / 房间），落点
      `src/data_interface/batch_worker.rs` 与 `src/lib.rs` 启动序，跑一遍留数。
- [ ] T03 在 `src/options.rs` 新增几何并发额度配置项（默认物理核数、最小 1、未知值启动失败），
      补解析回归。
- [ ] T04 新增全局几何并发闸模块（建议 `src/fast_model/concurrency.rs`），提供取额度与
      RAII 许可；补额度 = 1 时串行执行的纯函数测试。
- [ ] T05 `src/fast_model/manifold_bool.rs` 的固定 16 路 fan-out 改为从闸取额度；
      额度 = 1 与改前逐表等价。
- [ ] T06 [P] `src/fast_model/occ_generate.rs` 的 6 处 fan-out 接入闸。
- [ ] T07 [P] `src/fast_model/pdms_inst.rs` 的 4 处 fan-out 接入闸。
- [ ] T08 [P] `src/fast_model/gen_model.rs` 的 spawn/join 接入闸。
- [ ] T09 加源码形状断言：`src/fast_model/` 内不得再出现写死的并发宽度，新增 fan-out 必须过闸。
- [ ] T10 `src/data_interface/staging/write_context.rs` 增加每生成根的本地 journal buffer，
      根结束时整段并入窗口 journal。
- [ ] T11 `src/data_interface/staging/executor.rs` 增加封本前稳定序排定（根 ID → 表 →
      record id），排序最小单位为条目组，保住 ADR-038 第 2 条的显式事务原子性。
- [ ] T12 补确定性回归：同一输入两次运行的 journal 条目序列、`journal_digest`、
      ADR-038 分块边界与块指纹一致。
- [ ] T13 `src/fast_model/room_model.rs` 的逐房间判定接入闸，保持全量重建语义。
- [ ] T14 `src/fast_model/spatial_state.rs` 相关的分页读接入闸；换树 / 发布段仍持
      `SPATIAL_STATE_SERIAL`，锁序回归钉子同步更新。
- [ ] T15 [P] 并行下单根死信不影响同片其余根封本提交的回归测试
      （`src/data_interface/model_update_pending.rs`）。
- [ ] T16 [P] 房间收敛口径「对已有 mesh 的元素全部定论」的回归钉子
      （`src/fast_model/room_model.rs`）。
- [ ] T17 额度 = 1 与额度 = 核数各跑一次全量基线，逐表比对
      （`pe`、noun 属性、`ATT_UDA`、`pe_owner`、`inst_relate/info`、`geo_relate`、`inst_geo`、
      `world_trans`、`aabb`、空间树快照、`room_panel_relate`、`room_relate`），
      记录 wall-clock 与分段归因。
- [ ] T18 [P] 更新 `changelog.md` 与 `docs/2026-08-12_live-test-ledger.md`。
- [ ] T19 运行 `cargo fmt`、`cargo check --tests`、相关 feature 单测与 isolated live 测试。
- [ ] T20 执行 `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`，结果留证。

- [x] T21 模型认领页与执行组解耦：100 根 claim、默认 16 根 execution group，根锁只覆盖
      即将执行的小组；组间出现数据工作/epoch变化即停止接纳，未开始根不增加 attempts。
- [x] T22 `occ_generate` 后半程改为有界根级流水，单根失败逐根回报，健康根直接收口；
      Shape writer 与 AABB 串行纪律不变，并暴露 geometry/Shape 并发遥测。
- [x] T23 当前水位生成根凭证接入启动与 `model_ready`，缺凭证根自动补入既有队列；新增
      watch-scope 内指定 dbnum 的幂等全量模型重建 API。
- [ ] T24 在独立 SurrealDB 2.1.x 7997 快照上完成三轮 legacy / 三轮 adaptive A/B、语义
      hash、RSS/DB p99/写入量硬门和最终收敛；未完成前不得宣称性能发布门通过。

`[P]` 仅表示文件所有权互不重叠时可并行。T05 必须先于 T06–T08：先在一处验证「额度 = 1 与
改前等价」，再铺开。T02 必须先于 T17，否则加速比没有对照组。
