# 2026-08-19 剩余增量更新 live 用例续测

## 范围与环境

- 工作树：`D:\work\plant-code\old\gen-model`，提交基线 `0fe3f000`（工作树原有并行改动保留）。
- 测试库：仓库自带 SurrealDB 2.x，隔离 testbed `ws://127.0.0.1:8019`；TTY 清单覆盖另以
  `.surreal/ams-7997-e3d-test-20260805` 临时挂到 `8090`，执行结束即停止。
- 配置：`DB_OPTION_FILE=python/testbed/DbOption-pytest`，live 日志均保存在 `output/`。
- 本轮目标是把台账中尚无近期结果的当前可执行用例逐项落地；测试名已从现行
  `cargo test -- --ignored --list` 重新枚举。两个历史 ProjAMS 全名在现行源码匹配 0 项，已在台账标为退役记录。

## TTY 增量结论

1. `scripts/e3d/Test-TtyNetWindow.ps1`：通过。真实会话 `230→231→232`，FTUB `24384/23262`
   的 `POS.U` 为 `2900→3400→2900`，合并净窗口业务变化归零，仅保留保存元数据
   `BRAN.CACHID`；回滚行为已验证。证据：
   `output/e3d-tty-net-window/20260819-082310/`。
2. `scripts/Test-AmsModelTypeCoverage.ps1 -Endpoint ws://127.0.0.1:8090 -RequireVerified`：通过。
   字面结果：`actual=58, manifest=58, verified=58, pending=0, no_geometry=0`。证据：
   `output/tty-coverage-isolated8090-20260819-084723/coverage.log`。
3. 对常驻 `8009` 的一次只读覆盖查询命中了另一工作区的数据切面（actual 84 vs manifest 58）；
   该结果仅证明端点目标不匹配，不纳入 AMS gold 覆盖结论。原始记录：
   `output/tty-coverage-existing8009-20260819-084706/`。

因此现行 TTY 模型类型清单没有 pending 项；已验证条目为 58/58，本轮未重复驱动全部历史绿例。

## 8019 当前 live 用例结果

### 通过

- `test_cal_distance`（14.6s）。
- `live_issue13_c3_on_demand_pending_is_invisible_to_its_own_database`（14.8s）。
- `live_issue7_probe`（14.0s）。
- `live_issue5_moving_the_fitting_moves_its_implicit_tubing`：修订测试计划断言后通过（5.39s），
  精确工作项为 BRAN `RegenRoot` + 靶件 `PostRegenAabb`，位移及恢复均完成。
- `test_cal_rooms`：修订切面计数与空间树前置后通过（227.06s）；重建空间树 42,343 条，
  识别 214 间房/229 块面板，写入 41,370 条成员边。
- `rebuild_room_membership_on_the_live_project`（206.63s）：空间树 42,341，room_relate
  `1→41367`（+41,366）。
- `generating_one_root_fills_geometry_aabb_and_tree`：根 `24384/24776` 为 `AlreadyAvailable`，
  geometry/AABB/tree 均为 `184→184`。

### 已执行并保留为夹具或断言问题

- `live_suppo_pending_is_actually_regenerated`：SUPPO `24384/25725` 在当前切面查询为 `None`。
- `live_incomplete_room_panels_enqueue_targeted_repairs`：当前切面缺陷面板计数为 0。
- `test_build_room_panels_relate_common`：当前返回 0 条关系，历史夹具断言为 6。
- `live_issue7_real_db_deleted_edges_come_back`：生产 drain 已先恢复位置并收口任务，末端旧断言仍要求消费 1 条，实得 0。
- `live_issue13_c2_moving_out_of_the_room_clears_membership`：起点没有目标房间归属边。
- `staged_regen_persists_tubi_mesh_and_boolean_before_advancing_watermark`：缺少本轮专用输入
  `AIOS_STAGED_REGEN_DB_FILE`，在进入窗口前终止。
- `staged_transform_follows_a_pure_pose_move`：核心链已执行成功——7997 的 105..106 窗口完成
  暂存、2 根生成、提交与水位推进；最终仅旧断言要求 warning=0，而实际回执含净窗口口径提示
  与两个未解析生成根提示。
- `staged_pane_replay_goes_through_the_kvmem_window`：上一项已把 7997 收口到 file/applied=106/106，
  探针运行时无待重放窗口。

批次原始报告：

- `output/live-batch/remaining-b-data-bound-20260819-083041/report.json`
- `output/live-batch/remaining-room-and-panel-20260819-083240/report.json`
- `output/live-batch/remaining-room-live-20260819-084116/report.json`
- `output/live-integration-remaining-20260819-083450/report.json`

## 测试修订

1. `src/data_interface/sesno_range.rs`：适配 `get_nearest_large_sesno` 的现行 `Option<i32>` 返回值，
   缺少后继会话时结束稀疏会话枚举。
2. `src/fast_model/room_live_issue7.rs`：issue #5 live 断言纳入生产计划已有的 `PostRegenAabb`。
3. `src/fast_model/room_model.rs`：房间/面板数量改为非空门，精确切面可由两个环境变量显式钉住；
   空间树前置改走 `load_project_tree_verified`。

## 验证

- `cargo test --locked --lib a_session_budget_cuts_only_on_real_session_boundaries ...`：1 passed，exit 0。
- issue #5 修订后重跑：1 passed，exit 0。
- `test_cal_rooms` 修订后重跑：1 passed，exit 0。
- `cargo build --lib --tests --features http_api`：exit 0。
- `cargo check`：exit 0（依赖既有 warnings）。
- `git diff --check -- <本轮文件>`：exit 0。
- 补丁、修改件、原件哈希、验证日志及已在沙箱副本执行的回滚：
  `output/live-batch/remaining-verification-20260819-085400/`。

测试库 8019 仅由本轮启动的进程在收尾停止；常驻 8009 与既有 E3D/控制台进程未改动。
