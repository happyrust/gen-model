# 启动数据支撑修复验证证据（2026-08-14）

## 验证对象

- 启动重扫可检出 `file_latest_sesno == applied_sesno > 0` 且 PE 零行、无匹配空基线凭据的追平幽灵水位。
- 合法空基线凭据随应用水位原子收口，避免零行库每次启动重解析。
- 生产缺省 `startup_autorun=true`；显式 `false` 仍保留 held 行语义。

## 自动化结果

### Rust 库级质量门

```powershell
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd
```

结果：exit 0；`726 passed; 0 failed; 87 ignored`。

```powershell
cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd
```

结果：exit 0。仅有仓库既存的 `performance_test`、`test_tubi_inst_relate`、`e3d_mcp` warning。

### CI 集成测试目标

同一 feature 口径逐个执行：

- `db8000_two_delete_fixture`：6 passed。
- `db_session_fixture_selfcheck`：15 passed。
- `db8000_session_pairs`：20 passed。
- `pdms_record_boundary`：3 passed。

四个目标均 exit 0。

## live：人工路径三阶段闭环

```powershell
$env:DB_OPTION_FILE='python/testbed/DbOption-pytest'
$env:AIOS_MANUAL_UPDATE_PROJECT='AvevaMarineSample'
$env:AIOS_MANUAL_UPDATE_DBNUM='7998'
$env:RUST_MIN_STACK='16777216'
cargo test --lib --features http_api data_interface::manual_update::live_tests::live_rollback_and_ghost_watermark_reinit_end_to_end -- --ignored --exact --nocapture
```

结果：exit 0；1 passed；33.92s。依次验证文件回退、未追平幽灵水位、追平幽灵水位，最终 PE 恢复且应用水位与文件一致。

## live：真实启动重扫入口

```powershell
$env:DB_OPTION_FILE='python/testbed/DbOption-pytest'
$env:AIOS_MANUAL_UPDATE_PROJECT='AvevaMarineSample'
$env:AIOS_MANUAL_UPDATE_DBNUM='7998'
$env:RUST_MIN_STACK='16777216'
cargo test --lib --features http_api data_interface::increment_manager::tests::live_startup_sweep_repairs_a_caught_up_ghost_watermark -- --ignored --exact --nocapture
```

结果：exit 0；1 passed；测试体 19.26s（命令总耗时 31.7s）。关键字面结果：

- 启动重扫发现 `applied_sesno=12` 的追平幽灵水位；
- 以 `sesno 1..=12` 的 held `apply_window` 首次导入窗口入队；
- 同 dbnum 人工触发放行，worker 报告“已按首次导入重建”；
- 任务终态 `succeeded`，PE 恢复，应用水位恢复为 12。
