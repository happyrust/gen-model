# 增量模型生成缺陷修复任务清单（Tasks）

状态：P0+P1 已实现（2026-07-24）；F7 已确认无需实现；F8 已实现，真实 E3D 源修改 E2E 待授权（2026-07-26）
对应：`docs/specs/incr-gen-fixes/spec.md`（需求）、`docs/specs/incr-gen-fixes/plan.md`（方案）

> 任务按修复项分组，标注建议顺序、涉及文件、验收挂钩。`[P]` = 可与同组其它任务并行。
> 每个修复项完成后：`cargo build` + 相关单测通过，再勾选「验收」。

## 实现进度（2026-07-24）

- **已实现（代码）**：F1、F2、F3、F4、F5、F6、F8（持久化模型计划与反向索引消费）。
- **编译验证**：`cargo check --lib` ✅、`cargo check --tests` ✅、`cargo check --features occ` ✅；`cargo test --lib` 99 通过 / 5 失败（全部为**需要实时 SurrealDB 连接/权限**的既有集成测试：`room_model`、`team_data`、`manifold_bool`、`test_performance`，均非本次改动文件）。
- **不影响**：`--features "sql mqtt"` 有 8 处**既有**编译错误（aios_core API 漂移：`get_project_pool/get_global_pool/get_project_pools`），全在 `team_data.rs` / `versioned_db/database.rs`——非本次改动文件，属既有问题。
- **待办**：仍需真实 E3D 源修改的 Live 验证（T804 的源 session/UI 触发）。
  T106/T205/T306/T403/T605 已于
  2026-07-26 在本地 SurrealDB 通过；纯逻辑单测 T402 同日通过；
  F7 同日确认无需实现；P3 的 T902 同日核实为早已解决。
- **涉及改动文件**：`src/fast_model/gen_model.rs`、`src/fast_model/occ_generate.rs`、`src/data_interface/helper.rs`、`src/data_interface/model_refresh.rs`、`src/data_interface/side_effect_pending.rs`、`src/data_interface/increment_manager.rs`、`../pdms-io/src/io.rs`。

## 批次 P0（本期必修）

### F2 · mesh panic → 错误传播（先做，打通失败通道）
- [x] T201 `src/fast_model/gen_model.rs`：增量分支 `process_meshes_update_db_deep(...).await.expect(...)` → `?`
- [x] T202 `src/fast_model/gen_model.rs`：全量分支 `process_meshes_update_db_deep(db_option, &sites)` 同步改 `?`
- [x] T203 增量热路径 `.unwrap()/.expect()`：`occ_generate.rs` 的 `gen_inst_meshes`/`update_inst_relate_aabbs_by_refnos`/入口查询已改 `?`；`save_instance_data`（并行版，增量实际用）本就聚合写错误为 `Err`
- [x] T204 当前架构确认：模型计划与水位在同一事务持久化；生成失败由
  `model_update_pending` 标记 failed，旧 `SideEffectCompensator::ModelRefresh` 仅兼容历史记录
- [x] T205 [P] 本地 Surreal 集成测试**已通过**（2026-07-26）：
  `live_generation_failure_keeps_pending_and_watermark` 连续注入批量与逐根生成失败，
  断言进程不崩、根任务 `status=failed/attempts=1`、`applied_sesno` 保持 42
- [x] 验收 F2：代码路径与 Live 故障恢复验证均满足

### F1 · 删除元素几何孤儿清理（依赖 F2 通道）
- [x] T101 `model_refresh.rs`：`collect_deleted_geometry_refnos` 收集净变化 Deleted（跳过 SYS meta）
- [x] T102 `helper.rs::delete_inst_relate_subtree`：遍历被删 refno 的 pe 子树（含 deleted），收集自身+后代，调用幂等 `delete_inst_relate_cascade`
- [x] T103 `conservative_regen` 先 `cleanup_deleted_geometry` 再 owner 重生成；清理失败 `?` 上抛（走 F2 通道）
- [x] T104 子树遍历采用分批（20）的无深度上限 BFS；查询失败上抛并保留 durable pending，
  不再退化为“仅删根仍报成功”
- [x] T105 [P] 单测：净变化 = Deleted / Cancelled → 删除集分类正确（可加纯逻辑单测）
- [x] T106 [P] 集成测试**已通过**（2026-07-26）：`model_refresh.rs` 的
  `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling`——用 `4000000001/…` 造
  「软删父 + 软删子 + 未删兄弟」三棵几何，断言被删子树（含后代）清空且兄弟原样保留。
  跑：`cargo test --lib live_cleanup_by_pe_state -- --ignored --nocapture`（1 passed）
- [x] 验收 F1：代码路径与本地 Surreal Live 验证均通过

### F3 · 统一生成根归一（主/兜底/补偿一致）
- [x] T301 复用权威 `resolve_significant_owner`（= 生成根归一）作为单一实现
- [x] T302 `owner_regen`/`compensate_owners` 改用 `resolve_significant_owner`，删除 `noun==SITE||ZONE { continue }` 粗跳过
- [x] T303 回归 `compensate_owners` 调用点（`side_effect_pending.rs`）：行为一致
- [x] T304 补偿路径 deleted 清理：新增 `cleanup_deleted_by_pe_state`（按 pe.deleted 反推），drain 时先清理再 regen；`owner_regen` fallback 亦调用 `cleanup_deleted_geometry`
- [x] T305 [P] 单测：主路径与补偿路径生成根集合一致（对拍，可加）
- [x] T306 [P] 本地 Surreal 集成测试**已通过**（2026-07-26）：
  `live_zone_owned_equi_pending_is_actually_regenerated` 确认 `EQUI 24381/100677`
  直属 ZONE，经 durable pending drain 后任务清空且子树实际生成 17 个模型实例
- [x] 验收 F3：代码路径已满足；Live 验证待补

## 批次 P1（本期应修）

### F4 · `pe_owner` 幂等
- [x] T401 `../pdms-io/src/io.rs` `to_surql` 的 `Add` 分支：`INSERT RELATION INTO pe_owner` 前拼 `DELETE pe:{id}<-pe_owner;`
- [x] T402 [P] 语句级单测：`../pdms-io/src/io.rs` 新增 `add_relate_idempotency_tests`（3 个用例）——
  含 children 的 `Add` 渲染出的 SQL 里 `DELETE pe:{id}<-pe_owner` 必须早于 `INSERT RELATION`；
  同参数重复渲染字节一致；无 children 时完全不触碰 `pe_owner`。
  运行：`cargo test -p pdms_io --lib add_relate_idempotency`（须在 gen-model 工作区跑，
  pdms-io 单独构建会因 `parse_pdms_db` 的 gitee revision 失效而失败，gen-model 的
  `[patch]` 把它指向 `vendor/aios-parse-pdms`）。3 passed（2026-07-26）。
- [x] T403 [P] 集成测试**已通过**（2026-07-26）：`increment_pipeline.rs` 的
  `live_add_pe_owner_replay_is_idempotent`——取**真实 `to_surql` 输出**里的 `pe_owner`
  两句连跑两遍（模拟同窗口重放），断言第二遍不报错且关系数恰为 children 数、不重复累积。
  只回放关系语句、不回放 pe/noun 主记录，使断言不依赖属性载荷完整度。
  跑：`cargo test --lib live_add_pe_owner_replay -- --ignored --nocapture`（1 passed）
- [x] 验收 F4：代码路径与本地 Surreal Live 验证均通过

### F5 · SurrealQL 转义统一
- [x] T501 `../pdms-io/src/io.rs`：新增 `escape_surql_str`，`to_modify_surql` 的 NAME 两处已转义
- [x] T502 `gen-model` 复核：`update_datacenter_version` 仅插值枚举/数字，无外部字符串注入面（无需改）
- [x] T503 [P] 单测：名字含 `'` `\` 中文 → 语句正确、可落库（可加）
- [x] 验收 F5：代码路径已满足

### F6 · 自动路径接入文件异常检测
- [x] T601 `init_watcher`：按 dbnum 聚合（seen/blocked）支持 Duplicate 判定；`async_watch` 批内同款
- [x] T602 每个文件 `DbnumState::record_scan`（只写观察字段）—— 新增 `scan_and_check_file` 助手
- [x] T603 复用 `check_file_against_state`：`Rollback` → 跳过+告警；`PathMigrated` → record_scan 写新路径+告警
- [x] T604 `async_watch` 路径同款接入（与 `init_watcher` 对齐）
- [x] T605 [P] 集成测试**已通过**（2026-07-26）：`dbnum_state.rs` 的
  `live_record_scan_never_moves_the_applied_watermark`——建立水位 50 后，先扫到「路径已变 +
  更新会话 60」再扫到「更旧会话 10」，断言两次都只更新身份/观察字段、`applied_sesno` 恒为 50，
  并对拍纯判定给出 `Rollback`。判定口径本身已有 12 个纯函数单测覆盖。
  2026-07-26 已修正 async watcher 只看当前事件批次的问题：每次处理前重扫全部非递归监控
  文件并阻断跨事件重复 dbnum；`live_watch_directory_blocks_duplicate_dbnum_files`
  从真实 E3D 文件复制两个相同 60-byte 头到临时监控目录，确认目录扫描阻断该 dbnum。
- [x] 验收 F6：水位、回退与真实双文件目录扫描均通过

### F9 · durable 队列同轮完整消费
- [x] T906 `model_update_pending` 不再只取 50 条；watcher 先消费
  Transform/DeleteCleanup/CascadeExpand，再重新读取并批量消费其新入队的全部 RegenRoot
- [x] T907 非重生成任务失败时仍尝试已有/已展开 RegenRoot；两阶段错误均保留并汇总上报
- [x] T908 `incr_side_effect_pending` 去除 50 条截断，单次 init/watch drain 覆盖全部可重试任务
- [x] T909 本地 Surreal Live：构造 51 条 DeleteCleanup，单次 drain 返回 51 且队列清空
- [x] 验收 F9：共享 SPCO 的 67 个 BRAN 根不会等待无关文件事件才继续

## 批次 P2（排期）

### F7 · datacenter Add（已确认：不需要）

**结论（2026-07-26，源码取证）：gen-model 不应增加 `Add` 分支。** 依据：

1. `datacenter_version` 存的是**发布成功之后**的交付记录，不是 E3D 元素台账——
   `DataCenterRecord` 的文档注释原文「发布成功后的元数据，只存放最小交付单元」
   （`rs-core/src/data_center.rs:649`）。
2. 状态枚举 `DataCenterRecordOperate`（`rs-core/src/data_center.rs:641`）只有
   `Insert` / `Modify` / `Delete`，**没有 `Add`**；新元素对应的状态叫 `Insert`。
3. 记录的创建归发布流程管：`DataCenterRecord::get_insert_sql`
   （`rs-core/src/data_center.rs:661`）用 `upsert … set status = 'Insert'` 建记录，
   由 rs-server 的 datacenter 发布链路调用（`old/rs-server/src/datacenter/increment.rs`）。
4. gen-model 只发 `update`，按 `increment_pipeline.rs:737` 的注释，UPDATE 只命中已存在的
   交付记录；新增元素尚未发布、没有记录，UPDATE 必为空操作。

因此若在 gen-model 加 `Add` 分支：发 UPDATE 是纯噪音（必然空操作）；发 UPSERT 则会在元素
尚未发布时凭空造出交付记录，污染中台交付台账——两种都不对。**`Add` 的语义归发布流程，不归增量链路。**

- [x] T701 与业务确认新增元素是否需要 `datacenter_version` 的 `Add` 状态；结论如上
- [~] T702 取消：不需要 `EleOperationDetail::Add(_)` 分支（理由见上）
- [~] T703 取消：随 T702 取消
- [x] 验收 F7：结论在案，`_ => {}` 忽略 `Add` 是正确行为，代码无需改动

### F8 · CATA/规格反向传播（需独立 ADR）
- [x] T801 撰写 `docs/adr/ADR-008-catalog-reverse-propagation.md`（反查来源、触发范围、限制）
- [x] T802 实现：对共享 CATA/规格元件用 `ref_rev` 反查引用实例并入共享生成根计划
- [x] T803 反查非致命：失败降级告警；模型计划在水位前持久化并可重试
- [x] T805 触发重接线（2026-07-26）：`build_model_update_plan` 收紧为 DESI-only 时曾把
  CATA 触发一并断开；现改为 CATA 专用轻量分支——只为净变化 Modified/Deleted 且影响模型
  的目录元素落 `CascadeExpand` 种子（无 rollup / Transform / DeleteCleanup），执行器
  live 反查展开。`expand_live_reverse_cascade` 同步只对设计库引用者产根，目录/规格
  中间层只上溯，防止目录 owner 链被误当 Normal 根产生永败任务。
- [~] T804 [P] 下游 Live 已通过（2026-07-26）：
  `live_shared_spco_cascade_regenerates_every_consumer` 对共享 `SPCO 23274/295504`
  单次 drain 完成 1 个 CascadeExpand + 67 个 BRAN 根，队列清空且 72/72 个 DAMP
  消费者均存在模型（585.32s）。仍缺 E3D 中实际修改 SPCO 后的源 session/UI 触发证据
- [x] 验收 F8：纯逻辑、持久化、Live 反查与实际重生成均通过；源编辑/UI 触发待授权

## 批次 P3（卫生 backlog，非阻断）
- [~] T901 [P] D1：热路径 `dbg!/println!` → 分级日志（本次顺带移除了 `compensate_owners` 的 `dbg!(&owner)`；其余保留）
- [x] T902 [P] D2：**核实后确认已解决**（2026-07-26）。`get_inst_relate_nodes_in_subtree`
  （`increment_manager.rs:1420`）现在把子树收集委托给 `helper::collect_pe_subtree_refnos`
  （`helper.rs:17`），后者是 `while !frontier.is_empty()` 的**无深度上限 BFS**，靠
  `all.insert(refno)` 去重天然防环，分批 `SUBTREE_QUERY_BATCH = 20` 只控 SQL 长度、不截深度。
  原 backlog 描述的「硬编码 10 层」出自旧实现，已不存在（`delete_inst_relate_subtree(&[root], 10)`
  里的 `10` 是 chunk_size，不是深度）。
  全仓另外两处深度常量与本项无关且都是有意的防环保护：`manual_update.rs:61`
  `MAX_ANCESTOR_DEPTH = 32`（向上走 owner 链，注释已说明只防环）、
  `cata_closure.rs:220` `.max_depth(8)`（目录闭包）。
- [x] T903 [P] D3：已评估并修复。容量 1 通道对 PollWatcher 反压，处理期间的多次写入由
  下一轮文件头 + `applied_sesno` 合并补齐，不丢 session；另修复跨事件同 dbnum 双文件
  逃逸，事件处理前按全部监控文件重算重复集合
- [ ] T904 [P] D4：`raw_dchc_code` 覆盖度（已知限制，跟踪即可）

## 完成定义（DoD）
- 对应修复项的所有非 `[P]`-可选任务勾选完成。
- `cargo build`（含相关 feature）通过；新增/相关单测通过；不破坏现有测试。
- 每个修复项的「验收」逐条对照 `spec.md` 通过（Live 项在本地 Surreal+E3D 环境验证并记录）。
- 若引入行为变化，更新对应 `docs/adr` 或在 `spec.md`/本文件记录决策与限制。
