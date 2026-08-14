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
