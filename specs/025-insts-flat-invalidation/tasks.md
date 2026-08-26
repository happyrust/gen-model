# 025 `insts_flat` 失效协议任务

勾选口径：`[x]` = 在工作树里指得出对应代码或证据，`[ ]` = 指不出。
`[P]` 仅表示文件所有权互不重叠时可并行。

## 阶段 0 —— 前置：先把反例做成会红的测试（不写生产代码）

- [x] T01 在 `src/fast_model/pdms_inst.rs` 的 live 测试区新增共享 geo 反例：两个
      `inst_relate` 行共用一个 `inst_geo`，首轮该 geo `bad=true / meshed=false`（两行
      `insts_flat` 都不含它），随后只对其中一行做定向重生成（走 `render_inst_geo_upsert`
      第三个参数为真那条路），断言另一行的 `insts_flat` 不得停在旧值。
      前置条件写进测试名。
      → `live_shared_geo_bad_retry_must_refresh_sibling_insts_flat_on_disposable_db`
      （pdms_inst.rs:2042），2026-08-24 于 8019 内存沙箱**按设计红**（A 停在 `[]`，
      B 回填出 `["20260823"]`）。T18 闭环后转绿。
- [x] T02 判读并留证到 `docs/evidence/2026-08-23-insts-flat-invalidation/`：
      **能复现** → FR-6 选持久 pending 表（选项 P），FR-7 在 A/B 里择一；
      **不能复现**（现有调用序确实保证同批重建）→ FR-7 降为源码顺序断言，FR-6 选纯
      内存失效集（选项 V）。结论写进
      `specs/025-insts-flat-invalidation/plan.md` 的 R1 下面。
      → **能复现**：FR-6 定选项 P，FR-7 定路线 B（反向失效）。留证
      `t01-shared-geo-counterexample.md`（同目录），结论已入 plan.md R1。
- [ ] T03 [P] 只有在 T02 结论指向「`flat_valid` 布尔列 + 索引」这条备选时才做：
      `DEFINE INDEX` + `EXPLAIN FULL` 双引擎验证 2.1.4 的 planner 真的走索引
      （照 `anc CONTAINS` 那次的模板）。选项 P 不需要索引，本条可跳过并注明。

## 阶段 1 —— 止血（不依赖阶段 0 结论，可先走）

- [x] T04 `src/fast_model/pdms_inst.rs` `sweep_inst_relate_flat` 回填段：布尔分支判定
      改为与修复/脏值段同一个谓词（`!= NONE && != '' && lowercase(… ?? '') != 'none'`）。
      补一条回归：`booled_id = ''` 的行回填后 `insts_flat` 不得是 `[{ geo_hash: '' }]`
      （FR-10 / 验收 4）。**这是正确性修复，不是性能项。**
      → 已随提交 `cf7ec05d` 落地：共享判据 `VALID_BOOLED`（pdms_inst.rs:223）+
      源码形状回归 `both_sweep_segments_share_one_valid_booled_predicate`。
- [x] T05 同文件脏值计数段改 `LIMIT 1` 只判有无（FR-11）。
      → 已随提交 `cf7ec05d` 落地（只探有无，pdms_inst.rs:301）+ 回归
      `the_junk_probe_is_bounded`。
- [x] T06 修复段（Spec 019 那一段）改为带库上标记的一次性 migration：
      「标记不存在 → 修复 → 复核无残留 → 落标记」。标记名进语句字面量时过
      `dbnum_state::escape_surql_str`。同批更新
      `specs/019-booled-flat-backfill-closure/spec.md` 的状态注记（FR-9）。
      → `run_booled_flat_repair_migration_on`（pdms_inst.rs），标记
      `queue_control:booled_flat_repair_migration`；顺序钉
      `booled_flat_repair_migration_marks_only_after_a_clean_recheck` +
      行为钉 `booled_flat_repair_migration_marks_once_and_reruns_when_the_marker_vanishes`
      （mem，进 CI）+ 双跑补标记语句形态；019 spec 状态注记已加。
- [x] T07 [P] `src/test/fork_surreal_compat.rs`：T04 改后的回填语句形态在 mem / fork
      2.1.4 上双跑一致（NFR-5）。
      → 双跑语句已同步到 `VALID_BOOLED` 形态；2026-08-24
      `dual_inst_relate_flat_materialization_agrees` 与
      `dual_booled_flat_repair_converges` 双引擎均过。

## 阶段 2 —— 失效集与点名刷新

- [ ] T08 `src/fast_model/pdms_inst.rs`：`INSTS_FLAT_DIRTY: AtomicBool` →
      `flat_invalidated_refnos` 集合；`mark_insts_flat_dirty()` 改为收 refno。
      取批用 `std::mem::take`，成功丢弃快照、失败并回活动集（FR-2）。
- [ ] T09 并发回归：刷新进行中同一个 refno 再次变脏，旧批成功后新标记必须还在
      （本 spec 的 plan R3）。纯函数层可测，不连库。
- [ ] T10 集合上界与溢出降级：超限告警 + 置「需要一次全表自愈」+ 清空集合（FR-3 /
      验收 7）。上界进 `src/options.rs`（照既有配置项形状，含非法值校验）。
- [ ] T11 调用点改造 —— `src/fast_model/pdms_inst.rs` 的 `save_instance_data` 尾：
      不再无差别置脏，改为**收集** `plan.written_refnos` 到本次生成任务的候选集
      （不立即消费，FR-4）。
- [ ] T12 调用点改造 —— `src/fast_model/occ_generate.rs` AABB 段尾：把 `target_refnos`
      （本批刚落 `aabb` / `aabb_d`、因而刚满足 `aabb.d != none` 的那批）并入候选集。
- [ ] T13 消费点搬到生成任务终态：mesh / OCC / manifold / AABB 全部完成且任务成功之后
      才刷新（FR-4）。用源码顺序断言钉住「`insts_flat` 写入点不早于几何终态」
      （验收 5）。
- [ ] T14 刷新实现：按 record id 点名
      `UPDATE [inst_relate:⟨…⟩, …] SET insts_flat = …, aabb_d = …, world_trans_d = …`，
      并保留 `insts_flat = NONE AND aabb.d != none` 的过滤（未满足的 refno 留在集合里
      等下一轮，**不得**在发起时一把清空——那会漏掉「写了行但 aabb 还没落」的一批）。
- [ ] T15 按 T02 结论落 FR-6 的载体：
      **选项 P** → 新建独立 pending 表（record id 即目标 refno），成功 UPDATE 与删除
      pending 同事务、重放幂等；按宪法 IV 给出可消费 / 可收口 / 可复活三条出路，
      **不得**并进 `model_update_pending`。
      **选项 V** → 仅内存集合 + 启动全量自愈，并在代码注释里写明它成立的前提是 FR-5。
- [ ] T16 `src/data_interface/batch_worker.rs` 空闲轮：`sweep_inst_relate_flat_if_dirty()`
      换成点名刷新入口；全表清扫只留 `src/lib.rs` 启动那一处与人工诊断入口（FR-1）。
- [ ] T17 源码形状门（本 spec 的 plan R2 / 验收 8），两道——(a) 空闲轮路径内不得出现
      `inst_relate` 全表谓词；(b) 写 `geo_relate` / `inst_geo.meshed` / `booled_id` 的
      语句渲染点必须同批产出失效 refno，新增写点要么接上要么进豁免表并写明理由。
      写法照 `src/fast_model/pdms_inst.rs` 里现成的 `include_str!` + `split_once` 惯例
      （`flat_cache_prefers_booled_mesh_over_positive_primitives` /
      `flat_sweep_repairs_stale_booled_rows` 是同一子系统的模板）。

## 阶段 3 —— 闭环、保险与验收

- [ ] T18 按 T02 选定的路线闭合共享 geo（FR-7）：
      **路线 A** → `geo_hash` 纳入 mesh 算法版本，终态 `bad` 永不在同一 hash 恢复；
      **路线 B** → `bad → meshed` 时反向找出引用该 geo 的 `inst_relate` 行，durable 置
      无效并入队刷新。T01 那条测试必须由红转绿，且回退到旧写法时重新变红（验收 3）。
- [ ] T19 FR-5 落成纪律并加守护：凡可能让已物化缓存变旧的写，同事务内要么写新值、
      要么置无效。补一条崩溃语义回归——中途 kill 后受影响行只能是「已物化」或
      「NONE 走兜底」，不得是非 NONE 的旧值（NFR-3 / 验收 6）。
- [ ] T20 [P] 读侧行内自检（FR-8 前半）：pass1 多取 `booled_id`，`booled_id` 有效而
      `insts_flat[0].geo_hash` 不符时当缓存未命中转 pass2。**改动在 plant-ui 侧的
      vendor rs-core，不在本仓**——本仓只出对拍口径与验收判据。
- [ ] T21 `insts_flat_ver` 版本位（FR-8 后半）。**先解决本 spec 的 plan R5**：上版本位会让
      全库存量行在读侧眼里立刻变旧、集体退化为 pass2。要么随 T06 的 migration 一次刷完
      再切读侧，要么让读者同时接受 `ver` 缺失与当前值。方案写进本文件再动手。
- [ ] T22 [P] NFR-4 三个指标：`flat_pending_count` / `flat_oldest_pending_age` /
      `flat_fallback_ratio`，接进 `/health`（照现有 `model_update_pending` 快照的形状，
      不额外加查询预算）。
- [ ] T23 验收 1：同一个库改动前后各跑一轮首次导入基线，排 `平表副本清扫` 与
      `模型结点更新耗时` 两组总和，比值从 2.9 : 1 降到 < 0.05 : 1。留证。
- [ ] T24 验收 2：读侧五口径（refno / owner / aabb / trans / insts 哈希）对拍一致；
      `live_sweep_inst_relate_flat_on_configured_db` 的覆盖复核仍为「无残留」。
- [ ] T25 [P] plant-ui 全场加载壁钟不得较 P4 的 2.73s 基准退化。
- [ ] T26 [P] 更新 `changelog.md`（中文，`### 新增` / `### 修复`）与
      `docs/2026-08-12_live-test-ledger.md`（T01 / T18 / T24 三条 live 用例的最近通过
      记录）。
- [ ] T27 `cargo fmt`、`cargo check --tests`、CI 口径单测
      （`--no-default-features --features ws,gen_model,manifold,project_hd`），
      以及相关 `#[ignore]` live 测试；证据落
      `docs/evidence/2026-08-23-insts-flat-invalidation/`。
- [ ] T28 `sigmap verify-plan specs/025-insts-flat-invalidation/plan.md`、
      `sigmap verify-ai-output`、`sigmap review-pr`，结果留证。

---

硬顺序：

- **T01/T02 必须先于阶段 2**——结论决定 FR-6 的载体，也可能把 FR-7 整条降级。
- **T18 必须先于宣称 FR-1 完成**：不要在一个尚未证明正确的失效协议上做性能优化。
- **T21 在 R5 的方案写下来之前不动手。**
- 阶段 1 与阶段 2 之间没有硬序，但 T04 是正确性修复，优先级高于任何性能项。

