# kv-mem 增量 / 模型更新 / 房间增量 三链路审核报告（2026-08-07）

**基准**：ADR-017（暂存写回，含 2026-08-06 修订与五缺陷闭环）、ADR-010（房间增量全部增补）、
`docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md` §1 不变量 I1–I6。
**方式**：逐文件人工读码（`staging/*` 全部、`batch_worker`、`increment_pipeline`、
`model_update_pending` / `model_update_plan`、`manual_update` 关键路径、`occ_generate`、
`increment_manager`、`room_model`、`aabb_tree`、`helper`、`surreal_retry`、`preload` 等），
每条结论有 file:line 取证。**取证行号基于审核时的 HEAD（f09ccda4）**。

**总体结论**：三条链路的主干与 ADR 一致、纪律严密（源码断言测试密度很高）。发现
**1 个 P0 正确性缺陷**（暂存 Transform 路径写路由泄漏）和若干低危项。

**闭环状态（2026-08-07 当日）**：P0 已按
`docs/plans/2026-08-07-staged-transform-write-routing-fix-plan.md` 修复——
指针 UPDATE 改走 `execute_model_write`（f6000804，随修暂存回归测试与源码钉）、
窗口中途隔离性探针落地（b6c74482）、死代码删除（41dfd685）、本报告与 ADR-017
台账补记随 W5 落盘。

---

## P0：暂存窗口内 Transform 的 world_trans 指针直写持久层【已修复】

**位置**：`increment_manager.rs:2255-2264`（`update_world_transforms` 里指针批量
UPDATE 用 `SUL_DB.query(batch_sql)` 直写）。

**触发链**：`batch_worker.rs:529` 窗口 scope → `execute_frozen_batch_body` →
`run_staged_non_regen_work`（`model_update_pending.rs:970-972`）→
`mgr.update_world_transforms`。纯位姿变更（挪 EQUI/PIPE/ZONE 等）在暂存窗口内必然走到。

**三个后果**：

1. **违反 I1 零落盘**：窗口计算期间对持久层写
   `UPDATE inst_relate:… SET world_trans = trans:⟨新hash⟩`。同函数里 trans 记录本体、
   AABB 都正确走了 `execute_model_write`（暂存+journal），唯独指针这一步漏了。
2. **悬空指针**：新 trans 记录只在暂存/journal 里，写回成功前持久层不存在。窗口执行
   期间（阻断则**永久**）这些元素的 `world_trans.d` 为 none，从
   `where world_trans.d != none` 的一切读者（viewer、几何查询、包围盒刷新、房间判定）
   里整行消失——正是 ADR-010 D9 级别的故障形态。
3. **提交后位置错位（D1 复活）**：暂存里 inst_relate 行的指针从未更新，窗口内的 AABB
   刷新（`occ_generate.rs:833` 读 `active_data_db()`=暂存）拿**旧变换**算包围盒 →
   `tree_box_changed` 判「没变」→ 房间变更不触发、提交的 aabb 是旧位置。终态：模型画
   在新位置，而 aabb/空间树/房间归属永久停在旧位置，直到该单元因别的原因重生成。

**修复**（f6000804）：指针批量 UPDATE 改走 `crate::surreal_retry::execute_model_write`
（顺带让直写模式获得冲突重试）；`update_world_transforms` 的子树展开之后段提取为
`refresh_world_transform_products` 以便在暂存窗口内单测。回归测试断言 journal 含
trans INSERT 与指针 UPDATE、暂存行改指新记录且可解引用、房间变更寄存进窗口、持久层
零写入（SUL_DB 刻意不连接的负向对照），另加「回退即红」源码钉。此前暂存 Transform
路径**没有任何测试覆盖**（room_fixture 无 Transform 用例），这正是它漏网的原因。

---

## 已核实成立（摘要）

### kv-mem 暂存链路

- ReplaySafe 校验器 R1–R4 完整（显式 id、拒 RELATE/rand/time::now/相对更新、pe_owner
  范围只认单 owner 形状，`replay_safe.rs`）；executor 三模式路由、TX_CHUNK 分块重放、
  中断重试收敛有测试钉住。
- 尾事务内容与顺序正确（`model_update_pending.rs:459-491`）：datacenter 语句 →
  durable pending upsert → 空间意图 + epoch bump（同事务）→ revision 条件收口 →
  水位（math::max 单调）→ attempts 清除 → 恢复记录删除，整体一个事务。
- 冻结吸收（调度器 `record_frozen_end` 吸收后继 absorbed_by_running）、attempts 只按
  「新会话触及的根」重置（`roots_touched_since`，`batch_worker.rs:943-967`）、窗口阻断
  记录/解除语义正确。
- fail-closed 到位：读路由 miss 报错不回落、子树闭包深度溢出探针
  （`preload.rs:453-504`）、DBNUM 水位预载失败废窗（commit 7181112c）、房间预载失败
  整轮保留 pending。
- 提交后收敛：SpatialReconcile durable 意图 + 出队门 + 房间轮门 + 无限重试
  （`side_effect_pending.rs:233-300`、`batch_worker.rs:200-212`）。
- 按需生成 409（try_lock 不排队）、先落 durable pending 再生成、锁域覆盖窗口全部触碰
  根且按窗口前态解析（有源码断言）。
- 生成产物写路径全部经 `spawn_db_write→execute_model_write` 路由
  （`pdms_inst.rs:18-22`），inst_relate 替换事务先删后插、tubi 行并入删除集（D13）。

### 模型更新链路

- 计划构建次序有断言钉死：partition → issue#5 两半改判 → mask → rollup → append
  （`model_update_plan.rs:720-742`），预览逐步复刻（`manual_update.rs:3348-3366`）。
- revision 收口按 `(action, target, revision)` 谓词寻址不按重算 id；批量收口失败不误标
  生成失败；fresh 根合批 + 批失败逐根回退；死信 + 人工复活端点语义正确。
- 房间结构触发（改名/搬迁 → RoomRecalcPanel + H-1 结构预载）落地。

### 房间增量链路

- 尽力而为：窗口内失败落 durable pending 不阻断；暂存房间轮「面板先元素后、刻意不吸收」
  有论证注释；H-1 fail-closed（映射不可见的整间目标保留 pending，纯 AABB 且旧归属为空
  放行）。
- 元素分支彻底脱离空间树（PanelIndex 库内几何），整间分支空树拒跑 + 全量重建 90% 覆盖率
  门（`room_model.rs:153-170`）；NoGeometry 与空集在类型上分开。
- 同轮吸收封闭性检查与元素分支候选同源、查询失败一律不吸收
  （`model_update_pending.rs:1594-1615`）。
- 删除路径双表双向清 + 摘树暂存延迟（`helper.rs:315-358`）；整间分支排除
  `staged_spatial_removals`（`room_model.rs:983-984`）。
- 空间树管理与 ADR-010 增补 4 一致：epoch 只随空间意图 bump、sidecar 相等才信文件、
  指针重建只读、原子写、脏位落盘——全部有回退即红测试。

---

## 其他发现（P2）

1. **`save_instance_data_single` 是死代码**（`pdms_inst.rs:77-443`，全仓无调用方）：
   整段 `SUL_DB.query(..).unwrap()` 直写 + zone_refno 回填，谁误用它就整体绕过暂存。
   **已删除（41dfd685）**。
2. **资源估算低估**：WHERE 集合写按 1 行计（`replay_safe.rs:108`，已有 ponytail 注记）
   ——大命中窗口的 Abandon 档位会迟触发。本期不动，留痕待实测驱动。
3. **预载整表扫描**：`preload.rs:161` 每窗口 `SELECT VALUE in FROM inst_relate` 一次
   （已注记）；量级增长时需要端点索引。本期不动。
4. **P5 验收缺位**：方案 §6 自述 T5.1–T5.5（live 隔离性探针/终态对拍/故障注入）未落地。
   本次 P0 正是「单元探针够不着的路由泄漏」。**T5.1 精简版已落地（b6c74482）**：
   `staging/parity.rs` 的 `snapshot_data_tables` + 真实窗口设施三形态语句（解析写 /
   Transform / regen 产物）的写回前 diff 探针；live 版仍归 P5。
5. `drop_database` 前未强制 `use_ns`（`lifecycle.rs:464-484`），依赖共享会话纪律；
   残库有 sweep 兜底，低风险。

## 未能核实

- aios_core（rs-core-pin 仓）内部：读路由的缓存隔离只在 gen-model 侧测试验证；
  `get_world_transform` 对暂存中已删元素的行为未查。
- fork SurrealDB 服务器行为以 `fork-surreal-compat` findings 文档为准，未实测。
- 全链路 live 行为（P5 缺位所指的部分）未实机验证——本次为纯读码审计；修复后的
  单元/集成验收为 `cargo test --lib` 与 `--features http_api` 两口径全绿。
