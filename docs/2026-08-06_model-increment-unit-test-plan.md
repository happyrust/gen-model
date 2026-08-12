# 模型增量更新 · 单元测试计划（专跑单测的执行台账）

日期：2026-08-06（基线数字为当日工作树实测）
被测仓：`D:\work\plant-code\old\gen-model`（crate `aios-database`，lib 目标 `aios_database`）

> **本文定位**：把「模型增量更新」链路上**现存的每一条单元测试**按链路环节分组编号，
> 给出每组的过滤命令、条数与覆盖的不变量，后续按本文选组跑、回写记录（§8）。
> 实机 E2E、plant-ui 视觉闭环**不在本文**——那归
> `docs/2026-08-04_data-model-queue-test-plan.md`（三阶段总纲）管。

## 0. 与既有文档的关系

| 文档 | 它管什么 | 本文与它的关系 |
|---|---|---|
| `2026-08-04_data-model-queue-test-plan.md`（v3 总纲） | 三阶段门禁（G0–G4）、实机/视觉判据、环境配方 | 本文只吃它的 L0 层与环境铁律；实机判据仍以它为准 |
| `2026-07-27_increment-update-complete-test-plan.md` | 数据阶段 S0–S13 细粒度台账 | 数据面测试在本文 U12 只按模块计数，不重抄逐条矩阵 |
| `docs/plans/2026-08-06-staged-increment-five-defect-closure-plan.md` | 五缺陷闭环（W1–W5）与合并门禁 | 本文 §4 把它的缺陷/不变量映射到**真实存在的测试名**，§7 记录它验收项里仍欠的单测 |
| `docs/adr/ADR-017-staged-increment-window-commit.md` | 暂存窗口提交的决策与约束 | §4 的不变量口径来源 |

**分层记号**（沿用总纲）：本文管 **L0/L1**（`cargo test --lib`，进程内，含 kv-mem 暂存实例，不连外部库）
与 **L2 的定靶单跑**（`--ignored` 的 live 用例，隔离副本库）。

## 1. 怎么跑（口径与命令）

```powershell
# 全量门禁（两条都要跑，回写 §8）
cargo test --lib
cargo test --lib --features http_api

# 按组跑（过滤器是「子串匹配」，组前缀见 §3，如 U7 的批次执行）
cargo test --lib -- data_interface::batch_

# 精确单跑一条 + 看输出
cargo test --lib -- --exact data_interface::batch_worker::tests::spatial_reconcile_is_the_gate_before_every_dequeue --nocapture

# live 用例（L2）：先用环境变量定靶到隔离副本库，再 --ignored --exact 逐条单跑
$env:DB_OPTION_FILE = "db_options/DbOption-e2e-8009"   # 不带 .toml；db_options/ 已有 e2e-8009 / issue7-e2e / issue10-e2e 三份
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_fixture_parity --nocapture
Remove-Item Env:DB_OPTION_FILE                          # 跑完清掉，别污染后续命令

# 数字漂移时重新校准总数（本文所有条数是 2026-08-06 快照）
cargo test --lib -- --list          # 末行 "N tests"；--ignored 加上去只列 live
```

**三条铁律**（继承总纲，任何用例不得违反）：

1. 不动实库：live 一律用数据目录副本定靶；
2. 一份数据只起一个带 worker 的进程（双消费者破坏 FIFO）；
3. 只用 `bin/surreal.exe`（2.1.4），PATH 上的 3.x 会把目录写成 format_version 7，2.1.4 从此打不开。

**判读纪律**：每轮必须同时记 `passed / failed / ignored` 三个数（§8）。
`cargo test --lib` 全绿 ≠ 增量更新可用——live 半边与实机判据不在本文范围内，宣称口径见总纲 §6。

## 2. 总盘点（2026-08-06 基线）

| 目标 | 总数 | 默认可跑 | ignored（live/L2） | 本轮实测 | 耗时 |
|---|---|---|---|---|---|
| `cargo test --lib` | 506 | 439 | 67 | **439 passed / 0 failed / 67 ignored** | 2.6s |
| `cargo test --lib --features http_api` | 510 | 443 | 67 | **443 passed / 0 failed / 67 ignored** | 2.6s |

日志：`output/logs/2026-08-06_unit-baseline-default.log` / `..._unit-baseline-http.log`。
`http_api` 只多 4 条 `web_service::` 测试（U13），其余完全同集。
整套进程内测试 3 秒内跑完——**选组跑是为了定位，不是为了省时间；改完照例全量跑一遍**。

## 3. 分组矩阵（链路环节 → 测试组）

组号按模型增量更新的数据流排序。「可跑/live」为该组默认可跑条数与 ignored 条数。
过滤器直接接在 `cargo test --lib -- ` 之后，可一次给多个（空格分隔，取并集）。

| 组 | 链路环节 | 过滤器 | 可跑/live | 覆盖什么 |
|---|---|---|---|---|
| U1 | 变更影响判定 | `data_interface::model_impact::` `fast_model::shared::` | 33/0 | noun×属性变化 → 数据/模型影响分类的全字典契约、DCHC 码、成员 diff（B-EVT 系列） |
| U2 | 窗口折叠与数据管线 | `data_interface::increment_pipeline::` | 19/6 | 会话窗口净变化折叠（fold）、datacenter 语句渲染、staged 解析只进 journal、恢复记录（attempt）准入 |
| U3 | 交付单元与反向级联 | `data_interface::generation_root::` `data_interface::manual_update::` | 90/5 | 净变化合并（add/modify/delete 抵消）、交付单元归并/ZONE 汇总、ref_rev 反向索引与级联闭包、基线完整性判定、生成根锁注册表 |
| U4 | 模型工作计划 | `data_interface::model_update_plan::` | 13/4 | NetChange → ModelWorkItem 的分派（Transform/Regen 分流、去重排序、房间结构触发、CATA 级联播种、SYS 零工作） |
| U5 | durable pending 与三段 drain | `data_interface::model_update_pending::` | 32/11 | 待重试单元的复活/收口/revision 安全、尾事务（水位+空间意图+收口同事务）、房间任务寻址与吸收、死信保留 |
| U6 | 暂存窗口与写回 | `data_interface::staging::` `surreal_retry::` | 41/0 | 窗口生命周期/资源三级状态机/预载/ReplaySafe R1–R4/分块重放与尾事务/写上下文与根锁 guard/attempts/issue-10 夹具/直写对拍（parity） |
| U7 | 批次队列与执行 worker | `data_interface::batch_` `data_interface::task_registry::` `data_interface::side_effect_pending::` | 52/0 | 入队合并/冻结 FIFO/暂停语义/任务登记表、staged 批次骨架：根锁先于拷贝、房间预载 fail-closed、spatial 收敛先于出队、提交退避重试、合批准入 |
| U8 | 生成执行与产物落库 | `fast_model::gen_model::` `fast_model::occ_generate::` `fast_model::pdms_inst::` `fast_model::aabb_tree::` `fast_model::manifold_bool::` `fast_model::loop_model::` `fast_model::coverage_audit::` `fast_model::resolve::` `data_interface::model_refresh::` | 22/9 | 生成 worker 失败/panic 汇聚、AABB 变更判定与写序、inst_relate 原子替换、空间树脏标记与持久化失败、覆盖率审计 |
| U9 | 删除清理 | `data_interface::helper::` | 5/5 | 级联删除单事务、共享几何引用计数 GC、房间归属双向清理 |
| U10 | 房间归属 | `fast_model::room_` | 29/16 | 双表渲染同源（`room_relate`+`room_panel_relate`）、面板拓扑重写、整间/元素两分支、空树拒算、journal 准入；live 夹具全在此组 |
| U11 | 按需生成 | `data_interface::on_demand_model::` | 9/1 | durable pending 先行、活动根 409 拒绝、锁后查可用性、不可绘终态 |
| U12 | 数据面准入与兼容基线 | `data_interface::dbnum_state::` `data_interface::increment_manager::` `data_interface::update_scope::` `data_interface::project_paths::` `data_interface::cata_closure::` `versioned_db::` `test::fork_surreal_compat::` | 86/8 | 水位迁移/文件异常裁决、扫描准入与重复 dbnum、MDB 范围闸、监控目录、CATA 闭包、解析写管线、fork/mem 双引擎兼容 |
| U13 | Web 服务面（需 `--features http_api`） | `web_service::` | 4/0 | 身份三元组校验、静态资源降级、超时不取消后台任务 |
| — | 外围（不属于增量链，不设组） | `noun_layout::` `options::` `tables::` `data_interface::db_model::` | 8/2 | 布局字典、配置解析等；live 2 条是 `team_data`/`test_performance` |

校验和：可跑 33+19+90+13+32+41+52+22+5+29+9+86+8 = **439** ✓；live 6+5+4+11+9+5+16+1+8+2 = **67** ✓。

## 4. 不变量 / 缺陷 → 用例映射

五缺陷编号沿用 `2026-08-06-staged-increment-five-defect-closure-plan.md` §2；I7–I9 是该方案新增不变量。
**这里只列「钉住该不变量的主用例」，全名可直接 `--exact` 单跑。**

### 缺陷 1（I9 前半）：房间预载失败必须整轮 fail-closed

| 用例（全名） | 断什么 |
|---|---|
| `data_interface::batch_worker::tests::failed_room_preload_disables_the_staged_room_round` | 预载失败 → `room_map=None`，staged 房间轮不跑 |
| `data_interface::batch_worker::tests::only_design_windows_pay_for_room_preload` | 只有 DESI 窗口做房间预载 |
| `data_interface::model_update_pending::tests::staged_blind_panel_is_fail_closed_instead_of_cleared` | 看不见的面板宁可不算，不清空存量归属 |
| `data_interface::staging::preload::tests::room_working_set_is_staging_only` | 工作集只进暂存、不进 journal |
| `data_interface::staging::preload::tests::room_structural_targets_backfill_renamed_room_topology` | 改名房间的拓扑回填进工作集 |

### 缺陷 2（I7）：提交后的空间树与房间触发必须 durable

| 用例 | 断什么 |
|---|---|
| `data_interface::model_update_pending::tests::staged_tail_persists_spatial_intent_and_revision_guarded_settlement_before_watermark` | 尾事务：空间意图 + revision 收口排在水位之前、同一事务 |
| `data_interface::side_effect_pending::tests::spatial_reconcile_row_is_deterministic_and_keeps_final_net_mutation` | `spatial_reconcile` 行 id 确定、净变化保序 |
| `data_interface::side_effect_pending::tests::spatial_reconcile_rejects_conflicting_refno` | refresh/remove 两侧互斥 |
| `data_interface::batch_worker::tests::spatial_reconcile_is_the_gate_before_every_dequeue` | 出队前必先收敛（源码顺序断言） |
| `data_interface::batch_worker::tests::a_stale_spatial_tree_also_holds_back_the_room_round` | 树没收敛连房间轮也不放行 |
| `data_interface::batch_worker::tests::aabb_room_changes_only_become_durable_work_when_the_spatial_tree_is_on` | `gen_spatial_tree` 关闭时不落房间任务 |
| `fast_model::occ_generate::aabb_write_order_tests::*`（2 条） | AABB 记录先于指针持久化；关树时不进暂存窗口 |
| `fast_model::aabb_tree::tests::persist_failure_keeps_the_dirty_flag` / `marking_dirty_is_observable` | 树文件持久化失败保脏标记 |
| `data_interface::side_effect_pending::tests::spatial_reconcile_status_keeps_its_four_key_shape_in_both_branches` | /health `spatial_reconcile` 四键契约（pending/retries/last_error/stalled），成功与读库降级同键（G-02 收口，2026-08-12） |
| `fast_model::aabb_tree::tests::spatial_tree_status_keeps_its_nine_key_shape_in_both_branches` | /health `spatial_tree` 九键契约、指纹双字段漂移判定、降级不缩键（G-02 收口，2026-08-12） |
| `web_service::handlers::tests::health_routes_spatial_status_through_the_shared_renderers`（需 `http_api`） | health 接线纪律：两字段必须在、降级必须走共享渲染器而非 handler 手搓 JSON |

### 缺陷 3（I8）：Transform / DeleteCleanup / 按需生成统一根锁

| 用例 | 断什么 |
|---|---|
| `data_interface::batch_worker::tests::the_root_lock_closes_before_anything_is_copied_into_staging` | 闭包解析 → 持锁 → 拷贝的强制顺序 |
| `data_interface::batch_worker::tests::mutation_roots_resolve_against_the_pre_window_persistent_state` | 锁范围按窗口前持久层解析（暂存里查不到被删目标） |
| `data_interface::staging::write_context::tests::staged_generation_lock_lives_until_the_window_ends` | guard 活到窗口结束，不随批量提前释放 |
| `data_interface::staging::write_context::tests::a_second_hold_waits_instead_of_assuming_the_lock_is_already_ours` | 重复持锁等待而非假共享 |
| `data_interface::manual_update::tests::generation_lock_is_shared_by_root` / `generation_lock_registry_prunes_dead_keys` | 锁按根共享、注册表回收 |
| `data_interface::on_demand_model::tests::active_generation_root_is_rejected_instead_of_queued` | 命中持锁根 → 立即 409，不排队 |
| `data_interface::on_demand_model::tests::availability_is_checked_only_after_the_generation_root_is_owned` | 先锁后查，防窗口挤入 |
| `data_interface::model_update_pending::tests::pending_regeneration_holds_the_shared_root_lock_through_settlement` | drain 侧同一把锁贯穿收口 |

### 缺陷 4（I9 后半）：房间双表同源维护

| 用例 | 断什么 |
|---|---|
| `fast_model::room_model::tests::panel_topology_rewrite_clears_old_room_before_writing_current_room` | 面板拓扑重写先清旧房再写现房 |
| `fast_model::room_model::tests::both_room_panel_writes_are_admitted_into_the_window_journal` / `room_writes_are_journal_admissible` | 双表写入均过 ReplaySafe、可进 journal |
| `fast_model::room_model::tests::room_panel_write_clears_then_writes_addressable_edges` | `room_panel_relate` DELETE+INSERT、显式 id |
| `fast_model::room_model::tests::both_branches_address_the_same_edge_identically` | 整间/元素分支寻址同一条边 |
| `fast_model::room_model::tests::empty_member_set_still_clears_the_old_edges` / `room_with_no_panels_still_clears` | 空集也要清旧边（改名/不合规/迁移场景的渲染半边） |
| `data_interface::model_update_pending::tests::staged_removed_panel_clears_its_old_relations` | 不在册面板清双向关系 |
| `data_interface::helper::tests::deleting_an_element_clears_room_membership_in_both_directions` | 删除元素双向清边 |
| live 半边 | `fast_model::room_fixture::tests::live_room_*` 全系列（§5 门禁五连；改名转合规场景是 `live_room_rename_into_compliance_recomputes_membership`） |

### 缺陷 5：窗口成功根的存量 pending 收口（含 ADR-012 合批恢复）

| 用例 | 断什么 |
|---|---|
| `data_interface::batch_worker::tests::staged_fresh_units_join_batch_and_settle_only_in_finalize_tail` | fresh 根进合批、收口只走尾事务 |
| `data_interface::batch_worker::tests::only_fresh_parseable_revisioned_units_join_the_batch` | 合批准入三条件 |
| `data_interface::batch_worker::tests::staged_settlement_also_clears_pending_rows_this_database_never_recorded` | 别库登记的存量行也被收口 |
| `data_interface::model_update_pending::tests::batch_settlement_is_revision_safe_and_bounded` | revision 谓词收口、越界不动 |
| `data_interface::model_update_pending::tests::batch_settlement_failure_never_marks_generated_roots_failed` | 收口失败不给成功根记失败 |
| `data_interface::model_update_pending::tests::settlement_only_mutates_the_queue_revision_that_was_executed` / `settlement_addresses_the_row_by_its_fields_not_by_a_recomputed_id` | 收口寻址纪律 |
| `data_interface::manual_update::tests::worklist_merges_pending_with_new_units_keeping_latest_state` | 新单元与存量 pending 合并成一张工作单 |
| `data_interface::model_update_pending::tests::only_fresh_parseable_roots_join_the_regen_batch` | drain 侧合批准入同口径 |

### 窗口原子性与 journal 纪律（ADR-017 主干，兜底所有缺陷）

| 用例 | 断什么 |
|---|---|
| `data_interface::staging::replay_safe::tests::*`（7 条） | R1–R4：显式 record id、无随机、无时钟、无相对更新 |
| `data_interface::staging::executor::tests::*`（6 条） | validator 门口拒绝、分块重放顺序、断点重试收敛、放弃预算 |
| `data_interface::staging::lifecycle::tests::registered_finalize_commits_journal_and_watermark_together` | journal 与水位同批提交 |
| `data_interface::model_update_pending::tests::finalization_is_one_transaction_with_delivery_status_work_watermark_and_cleanup` | 直写路径尾事务同样单事务 |
| `data_interface::staging::parity::*`（2 条） | 暂存写回 ≡ 直写（mem 实跑对拍） |
| `data_interface::staging::issue10_add_node::*`（3 条） | 阻断→吸收解锁、毒写回→修复后重放收敛、连续窗口加节点 |
| `data_interface::batch_worker::tests::staged_commit_retries_with_backoff_until_success` / `staged_commit_stalls_without_discarding_then_recovers` | 写回失败退避重试、不丢暂存 |
| `data_interface::staging::routing_tests::staging_context_routes_reads_and_never_touches_sul_db` + `surreal_retry::generation_preload_is_staging_only_inside_a_window` | 窗口内读路由不碰持久层 |
| `data_interface::staging::attempts::tests::*`（3 条） | 根失败累计、吸收重置解锁、尾事务清 attempts 只清本 dbnum |

## 5. live 定靶台账（67 条，L2）

跑法见 §1；**逐条 `--exact` 单跑**，不要整包 `--ignored` 放行（互相抢库）。
定靶配置放 `db_options/`（现有 `DbOption-e2e-8009` / `DbOption-issue7-e2e` / `DbOption-issue10-e2e`），
新场景先复制副本库再新增 toml，不改仓库根 `DbOption.toml`。

| 模块 | 条数 | 干什么 | 前置 |
|---|---|---|---|
| `fast_model::room_fixture` | 11 | 房间夹具：parity/移动/增量/结构触发/删除/跨面板/吸收/TUBI/改名合规 | 隔离副本库 + mesh 目录可写 |
| `fast_model::room_live_issue7` | 2 | issue-7 删除边复活回归 | issue7 定靶库 |
| `fast_model::room_model`（`test_cal_rooms` 等 3 条旧式） | 3 | 手工探针，非断言型 | 真实工程库，仅诊断用 |
| `data_interface::model_update_pending`（`live_*`） | 11 | pending 真重生成（BRAN/HANG/SUPPO/EQUI）、finalize 崩溃安全/幂等、OS kill 恢复、共享 SPCO 级联 | 副本库；崩溃类用例会杀进程 |
| `data_interface::increment_pipeline`（`live_*` + bench + fold real window） | 6 | 真实会话窗口重放幂等、删除会话清模并重生成、FTUB 删移重排 | 副本库 + 对应 .db 样本 |
| `data_interface::manual_update`（`live_*`） | 5 | 全库基线、项目级手动更新、ref_rev 重建自检 | 副本库（会跑很久） |
| `data_interface::model_update_plan`（`live_projams_*`） | 4 | PROJAMS 实库计划/执行分派 | PROJAMS 样本工程 |
| `data_interface::helper`（`live_*` + `probe_live_sql`） | 5 | 实库级联删除/引用计数回收 | 副本库 |
| `data_interface::dbnum_state` / `increment_manager` / `update_scope` | 6 | 实库水位不动产、监控目录重复 dbnum 阻断、真实 MDB 声明 | 副本库 + 监控目录 |
| `data_interface::model_refresh` / `on_demand_model` / `coverage_audit` / `aabb_tree` | 6 | 按 pe 状态清子树、缺模型按需生成、未覆盖 noun 直方图、空间树从库同步 | 副本库 |
| `fast_model::resolve` / `manifold_bool` / `occ_generate::test_gen_geos` | 4 | SCOM 几何解析、布尔失败路径、真实几何生成 | 副本库（occ 重） |
| `versioned_db::member_prune`（2 条 real parse） | 2 | 真实解析的站点剪枝 | .db 样本文件 |
| 外围：`team_data::test_ancestor` / `test::test_performance::…` | 2 | 祖先链探针 / 性能剖析 | 真实工程库，不入门禁 |

**门禁五连**（合并主分支前逐个单跑，口径来自五缺陷闭环方案 §W5）：

```powershell
$env:DB_OPTION_FILE = "db_options/DbOption-e2e-8009"
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_fixture_parity --nocapture
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_panel_move_parity --nocapture
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_incremental_parity --nocapture
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_structural_triggers_enqueue_panel_recalc --nocapture
cargo test --lib -- --ignored --exact fast_model::room_fixture::tests::live_room_delete_clears_membership --nocapture
Remove-Item Env:DB_OPTION_FILE
```

崩溃恢复 / 空间持久化失败 / 生成竞争三类故障注入的 live 半边（W5 门禁第 4 项）：
`live_finalize_is_crash_safe_and_idempotent`、`live_os_kill_preserves_prepared_attempt`、
`live_generation_failure_keeps_pending_and_watermark`（都在 U5 的 live 里）。

## 6. 改哪跑哪（源文件 → 组）

改动落在左列文件 → 至少跑右列组；**之后全量 `cargo test --lib` 收尾**（反正 3 秒）。

| 源文件 | 主组 | 连带组 |
|---|---|---|
| `data_interface/model_impact.rs` | U1 | U4（分派吃它的分类） |
| `data_interface/increment_pipeline.rs` | U2 | U6（staged 解析）、U5（finalize） |
| `data_interface/manual_update.rs`、`generation_root.rs` | U3 | U5（工作单合并）、U4 |
| `data_interface/model_update_plan.rs` | U4 | U5、U10（房间触发） |
| `data_interface/model_update_pending.rs` | U5 | U7（drain 挂载）、U6（尾事务渲染） |
| `data_interface/staging/*`（lifecycle/preload/executor/replay_safe/resources/write_context/attempts） | U6 | U7（batch_worker staged 分支）、U5 |
| `data_interface/batch_worker.rs`、`batch_queue.rs`、`batch_scheduler.rs`、`task_registry.rs`、`side_effect_pending.rs` | U7 | U5、U6 |
| `fast_model/gen_model.rs`、`occ_generate.rs`、`pdms_inst.rs`、`aabb_tree.rs` | U8 | U10（AABB 房间触发）、U7（空间收敛） |
| `data_interface/helper.rs` | U9 | U10（房间边清理） |
| `fast_model/room_model.rs`、`room_predicate.rs`、`room_fixture.rs` | U10 | U5（房间任务寻址） |
| `data_interface/on_demand_model.rs` | U11 | U3（锁注册表） |
| `data_interface/dbnum_state.rs`、`increment_manager.rs`、`update_scope.rs`、`project_paths.rs`、`cata_closure.rs`、`versioned_db/*` | U12 | U2 |
| `web_service/*` | U13（记得 `--features http_api`） | — |

当前工作树未提交改动对应：`batch_worker.rs`→U7、`model_update_pending.rs`→U5、
`side_effect_pending.rs`→U7、`staging/lifecycle.rs`+`staging/preload.rs`→U6、
`gen_model.rs`→U8、`room_model.rs`+`room_fixture.rs`→U10。提交前这六组 + 全量两条门禁都要绿。

## 7. 已知缺口（要补的单测，按优先级）

| # | 缺口 | 出处 | 建议落点 |
|---|---|---|---|
| G-01 | `SideEffectCompensator::reconcile_spatial_pending` 的**跨行合并语义**（多条未完成任务合并 refresh/remove、同 refno 以较晚净变化为准）无进程内专测；W1.3 验收三条里「暂停时收敛照跑」「收敛失败不出队（行为级）」也只有源码顺序断言 | 五缺陷方案 §W1.3 验收 | `side_effect_pending.rs::tests`（合并语义）+ `batch_worker.rs::tests`（pause/失败行为） |
| ~~G-02~~ | **已收口（2026-08-12）**：~~`/health` 的 `spatial_reconcile` JSON 形状无专测~~ → 形状拼装抽成纯渲染函数（`render_spatial_reconcile_status` / `spatial_reconcile_error_status` / `render_spatial_tree_status`），handler 降级分支改走共享渲染器；四键/九键/接线三条测试已入 §4 缺陷 2 映射表，前两条默认特性即跑（CI 特性集可见） | 五缺陷方案 §W1.4 验收 | 已落 `side_effect_pending.rs` + `aabb_tree.rs` + `web_service/handlers.rs` |
| G-03 | `src/test/` 下 `test_spatial`（含 test_room 25 条）、`test_api`、`test_query`、`test_incr_update`、`test_data_state` 等模块在 `src/test/mod.rs` 被**注释掉**，不编译——它们不构成任何覆盖，别把 grep 到的 `#[test]` 当现役资产 | `src/test/mod.rs` 现状 | 定夺：恢复编译（挑有价值的）或删除死代码 |
| G-04 | W5 门禁的三类故障注入只有 live 半边（§5 末），未纳入常规轮换；崩溃类用例杀进程，需专用剧本与记录 | 五缺陷方案 §W5 | 按 §5 剧本每次合并前跑，结果记 §8 |

## 8. 轮次记录表（每轮回写）

| 日期 | 触发（改了什么/为什么跑） | 命令 | passed/failed/ignored | 备注与证据 |
|---|---|---|---|---|
| 2026-08-06 | 建台账基线（工作树含五缺陷闭环 WIP） | `cargo test --lib` | 439/0/67 | `output/logs/2026-08-06_unit-baseline-default.log` |
| 2026-08-06 | 同上 | `cargo test --lib --features http_api` | 443/0/67 | `output/logs/2026-08-06_unit-baseline-http.log` |
| 2026-08-12 | 关缺口 G-02（/health 形状单测 ×3；spatial_reconcile 降级改走共享渲染器） | `cargo test --lib` | **648/0/79** | `output/logs/2026-08-12_unit-g02-default.log`；总数自 §2 基线 506 漂移到 727（08-09 房间轮重构 `4ed921c3` 起 §3/§4 部分口径已过期，待整体校准） |
| 2026-08-12 | 同上 | `cargo test --lib --features http_api` | **657/0/79** | `output/logs/2026-08-12_unit-g02-http.log`；web_service 现 9 条（本文 U13 记 4 条已过期） |
|  |  |  |  |  |

回写规则：

- 每轮跑完在表尾追加一行；failed ≠ 0 时备注列写**第一个失败用例全名**与日志路径；
- 新增/删除测试导致条数漂移时，顺手更新 §2 与 §3 对应组的计数（用 §1 的校准命令）；
- live 轮（§5）另记定靶配置名（`DB_OPTION_FILE` 值），不记的视为没跑；
- §7 缺口关掉一条就划掉一行，并把新测试名补进 §4 对应映射表。
