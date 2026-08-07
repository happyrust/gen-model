# 修复开发计划：暂存 Transform 写路由泄漏与审核问题闭环（2026-08-07）

> 依据：2026-08-07 三链路读码审核（kv-mem 暂存增量 / 模型更新 / 房间增量）。
> 关联：ADR-017（稳态增量窗口暂存写回，不变量 I1/I2）、ADR-010（房间归属增量，D1/D9 教训）、
> `docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md` §6 落地情况。
> 本文只写「改什么、按什么顺序、怎么验收」。

## 1. 问题清单（按严重度）

### P0-1 暂存窗口内 Transform 的 world_trans 指针直写持久层

- **位置**：`src/data_interface/increment_manager.rs:2255-2264`（`update_world_transforms`
  的指针批量 UPDATE 用 `SUL_DB.query(batch_sql)` 直写）。
- **触发链**：`batch_worker::execute_frozen_batch` 窗口 scope →
  `execute_frozen_batch_body` → `run_staged_non_regen_work`
  （`model_update_pending.rs:970-972`）→ `mgr.update_world_transforms`。
  纯位姿变更（挪 EQUI / PIPE / ZONE …）在暂存窗口内必然走到。
- **后果**：
  1. 违反 I1 零落盘：窗口计算期间对持久层写 `UPDATE inst_relate:… SET world_trans = trans:⟨新hash⟩`。
  2. 悬空指针：新 trans 记录只在暂存/journal 里，写回成功前持久层不存在；窗口阻断/废弃时
     **永久**悬空——这些元素的 `world_trans.d` 为 none，从 `where world_trans.d != none`
     的一切读者（viewer、几何查询、包围盒刷新、房间判定）里整行消失（D9 级故障形态）。
  3. D1 复活：暂存里 inst_relate 行的指针从未更新，窗口内 AABB 刷新
     （`occ_generate.rs:833` 读 `active_data_db()`=暂存）拿**旧变换**算包围盒 →
     `tree_box_changed` 判「没变」→ 房间变更不触发、提交的 aabb 是旧位置。
     终态：模型画在新位置，aabb / 空间树 / 房间归属永久停在旧位置。
- **成因**：同函数里 trans 记录（`save_transforms_to_surreal`）、AABB 刷新都已走
  `execute_model_write` 路由，唯独指针 UPDATE 这一步在 ADR-017 接线时漏掉；
  暂存 Transform 路径没有任何测试覆盖（room_fixture 无 Transform 用例）。

### P2-1 `save_instance_data_single` 死代码

- `src/fast_model/pdms_inst.rs:77-443`，全仓无调用方；整段 `SUL_DB.query(..).unwrap()`
  直写 + zone_refno 回填。误用即整体绕过暂存路由，且 `.unwrap()` 风格与现行纪律相悖。

### P2-2 暂存隔离性缺少执行中途的整库探针（方案 T5.1 缺位）

- 写回方案 §6 自述 P5（T5.1–T5.5）未落地。本次 P0 正是「单元级探针够不着的路由泄漏」：
  漏网点不在被测模块里，而在被调用的旧函数内部。需要一道**窗口执行中途对持久层做
  数据表快照 diff** 的机械防线，堵住「下一次有人再漏接一处」。

### 记录项（不阻塞，随本计划一并处理）

- 审核报告落盘 `docs/2026-08-07_three-chain-audit.md`；ADR-017 落地台账补记本次修复。
- `estimate_write_rows` 对 WHERE 集合写按 1 行计、`preload.rs:161` 整表扫描：均已有
  ponytail 注记，本期不动，只在报告里留痕。

## 2. 工作包

### W1（P0-1 修复）：指针 UPDATE 改走模型写路由

- **改动**：`increment_manager.rs` `update_world_transforms` 中
  `SUL_DB.query(batch_sql)` → `crate::surreal_retry::execute_model_write(&batch_sql, "update world_trans pointers")`。
- **行为**：
  - 暂存窗口内：指针 UPDATE 进暂存库生效 + 进 journal（`ExecMode::Both`），
    窗口内 AABB 刷新立即读到新变换 → 房间变更判定恢复；持久层零写入。
  - 直写模式：从裸 `query` 升级为带写冲突重试的 `execute_surreal_checked`，
    行为只更稳，无接口变化。
- **顺带核对**（读码确认，不改代码）：`get_inst_relate_nodes_in_subtree` 的
  `SUL_DB` 直读保持不动——锁范围/子树按窗口前持久态解析是既定纪律
  （`mutation_roots_resolve_against_the_pre_window_persistent_state` 已钉）。

### W2（P0-1 回归测试）：暂存 Transform 的三条断言

新增暂存窗口单测（`increment_manager.rs` 或 `staging/` 测试模块，复用
`create_window_on` + mem 实例）：

1. **journal 断言**：窗口 scope 内跑 `update_world_transforms`（预载一行
   pe + inst_relate + 旧 trans 记录），journal 必须包含 trans 记录 INSERT 与
   指针 UPDATE；暂存库行的 `world_trans` 指向新 hash。
2. **零落盘断言**：扮演持久层的独立 mem 实例在整个过程中数据表零写入
   （对照快照 diff 为空）。
3. **房间触发断言**：位姿变化导致包围盒变化时，`deferred_spatial().room_changes`
   捕获到该 refno（D1 不复活）。

另加一条「回退即红」源码断言：`update_world_transforms` 函数体内不得出现
`SUL_DB.query` 的写调用（与本仓既有源码钉法同风格）。

### W3（P2-2）：窗口中途隔离性探针（T5.1 精简版）

- 测试侧工具：`snapshot_data_tables(db) -> BTreeMap<table, hash>`，
  排除控制面白名单（`dbnum_watermark` 观察字段、`increment_update_attempt`、
  `queue_control`、`model_update_pending`、`incr_side_effect_pending`）。
- 用 mini 窗口（复用 T0.6 harness 的形态：解析写 + Transform + regen 产物各一条）
  在「写回之前」对持久层实例 diff，必须为空；写回之后 diff 恰好等于 journal 终态。
- 不追求覆盖全生成管线，先把机械防线立起来；live 版 T5.1 仍留在 P5 待办。

### W4（P2-1）：删除 `save_instance_data_single`

- 直接删除函数与其专属辅助（确认无 `#[cfg(test)]` 引用后）；`cargo check` 守护。

### W5（记录）：报告与台账

- 审核报告全文落 `docs/2026-08-07_three-chain-audit.md`（含取证行号）。
- ADR-017「落地情况」补一行：暂存 Transform 指针写路由泄漏（P0）于 2026-08-07
  修复，回归测试与隔离探针随修。

## 3. 顺序与验收

1. W1 + W2 同一提交（修复与钉子不拆开）；
2. W3 独立提交；W4、W5 各自独立提交；
3. 验收：`cargo test --lib` 与 `cargo test --lib --features http_api` 全绿；
   新增测试在修复前必须能红（先写测试验证能抓到直写，再改代码）；
4. 可选实机验证（有 E3D 环境时）：真库挪一个 EQUI → 暂存窗口提交 →
   `inst_relate.aabb` 与 `room_relate` 跟随新位置；窗口人为阻断时持久层
   `world_trans` 无变化。

## 4. 风险与回滚

- W1 让指针 UPDATE 进 journal，journal 体量略增（每个 Transform 目标一条 UPDATE），
  相对解析/产物语句是噪音；资源门禁口径不变。
- W1 改的是单一调用点、无接口变化；回滚 = revert 该提交。
- W3 探针只存在于测试；W4 删除死代码不影响生产路径（无调用方已核实）。

## 5. 明确不做（本期）

- 不动 `estimate_write_rows` 的行数代理与 `preload.rs` 的整表扫描（已注记，待实测驱动）；
- 不实现 live 版 T5.2–T5.5（终态对拍/故障注入/性能基线），仍归 P5；
- 不改 `GEN_MODEL_DIRECT_INCREMENT` 直写紧急路径的语义。
