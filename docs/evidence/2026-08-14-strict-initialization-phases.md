# 严格分阶段初始化验证记录（2026-08-14）

## 验证对象

- 数据顺序：`Meta(SYS/DICT) -> Catalogue(CATA) -> Design(DESI)`。
- 派生顺序：数据就绪后才开放模型与 AABB，模型收口后才开放房间后处理。
- 阶段屏障：manifest/epoch、早期阶段回退、CATA 项目优先级、HTTP/Python 模型门。

## 本地质量门

所有命令均在 `D:\work\plant-code\old\gen-model` 执行。

| 命令 | 结果 | 退出码 |
|---|---|---:|
| `cargo fmt --all -- --check` | 通过 | 0 |
| `cargo check --locked` | 通过；依赖代码产生 204 条既有 warning | 0 |
| `cargo test --locked --lib data_interface::initialization_phase::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 10 passed | 0 |
| `cargo test --locked --lib data_interface::batch_queue::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 23 passed | 0 |
| `cargo test --locked --lib data_interface::batch_scheduler::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 11 passed | 0 |
| `cargo test --locked --lib data_interface::batch_worker::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 34 passed | 0 |
| `cargo test --locked --lib data_interface::increment_manager::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 25 passed, 3 ignored | 0 |
| `cargo test --locked --lib data_interface::manual_update::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 105 passed | 0 |
| `cargo test --locked --lib data_interface::model_update_pending::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 51 passed, 12 ignored | 0 |
| `cargo test --locked --lib data_interface::side_effect_pending::tests --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 9 passed | 0 |
| `cargo test --locked --lib versioned_db::database::sync_pdms_awaits_global_meta_then_catalogue_then_design --no-default-features --features ws,gen_model,manifold,project_hd -- --exact --nocapture` | 1 passed | 0 |
| `cargo test --locked --lib startup_room_build_gate_tests::startup_waits_for_data_then_models_before_rooms --no-default-features --features ws,gen_model,manifold,project_hd -- --exact --nocapture` | 1 passed | 0 |
| `cargo test --locked -p aios-py exec_api::tests::python_model_writes_and_room_postprocessing_observe_initialization_gates -- --exact --nocapture` | 1 passed | 0 |

## CI 集成目标与 Python 离线档

| 命令/目标 | 结果 | 退出码 |
|---|---|---:|
| `cargo test --locked --test db8000_two_delete_fixture --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 6 passed | 0 |
| `cargo test --locked --test db_session_fixture_selfcheck --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 15 passed | 0 |
| `cargo test --locked --test db8000_session_pairs --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 20 passed | 0 |
| `cargo test --locked --test pdms_record_boundary --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 3 passed | 0 |
| `python\\.venv\\Scripts\\python.exe -m pytest -m offline -q -p no:cacheprovider --basetemp D:\\Rust\\target\\pytest-strict-init-20260814-01` | 67 passed, 20 deselected | 0 |

## Oracle / Live 状态

- Oracle 已完成 dry-run 文件与 token 报告；浏览器审查会话 `strict-init-core-review` 因 Chrome 连接中断结束，未产生可引用审查正文。
- 六条计划中的破坏性 live 场景本轮未执行，也未写入 `docs/2026-08-12_live-test-ledger.md` 的“最近通过”栏；它们仍按仓库规则视为未验资产。

## test-worklspace 运行验证

在 `D:\work\plant-code\old\test-worklspace` 部署提交 `c4865ea5` 的 Release
二进制并连接 SurrealDB 2.x 沙箱。完整命令、健康快照、队列快照、字面输出及回滚
记录位于：

`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\strict-init-c4865ea-20260814-165740\verification.md`

已确认：

- 同项目 CATA 重复文件使初始化停在 Catalogue；Design 和模型均未执行。
- 解除重复后只派发 8 个 CATA，20 个 Design 保持未派发；首个 CATA 7351 完成后
  水位为 `116/116`。
- 发现运行态缺陷：周期 reconcile 在 7351 长批次运行期间把 manifest 从 epoch 2
  推进到 5；队列仍有 `epoch=2 state=running`，健康状态却报告 Catalogue
  `running=0, pending=8`，并把运行行显示为 `blocked_by_phase=catalogue`。无变化重扫
  不应替换 epoch，旧 epoch 在飞行也应持续投影到健康状态。

测试后已暂停队列，快删 7351（删除 252500 行 PE，水位恢复 0），并恢复原二进制、
配置和重复文件；运行端口 9099 已关闭。

## E3D TTY 增量闭环

使用 L3 TTY 驱动对 db8000 的 FTUB `24384/22403` 执行 OWNER 搬迁及恢复：

- apply 会话 221：owner `24384/22402 -> 24384/22404`；TTY 与离线 fold 均成功。
- restore 会话 222：owner 恢复为 `24384/22402`。
- watcher 把 221、222 合并为一个 Design 批次；任务
  `db-20260814-172754-000000` 成功，`changed_elements=6`，最终水位 `222/222`。

发现三项运行态问题：

1. 成功结果的 `merged_sesnos`/`merged_sesno_times` 均为空，未传递冻结窗口的
   `[221, 222]` 会话清单。
2. Design epoch 到来时，已 claim 的模型页中剩余 15 行被记成
   `initialization_not_ready` 失败且 attempts 从 1 升至 2；测试后已返还 attempts 并
   恢复 pending。
3. 数据批次等待已 claim 模型页约 317 秒，且同一逻辑变更的 epoch 从 2 反复推进到
   4、6、7。

完整字面证据及回滚记录：

`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\e3d-tty-increment-c4865ea-20260814-172431\verification.md`
