# 全新库自动基线验证证据（2026-08-17）

## 验证对象

- 范围内**从未解析**的 dbnum（无水位行、无 `dbnum_info_table` 统计行、无 pe 行——
  对应「新库文件第一次进入监控目录」）必须被启动重扫自动发现、入队并由 worker
  的 `needs_initial_load` → `initialize_dbnum_baseline` 基线分支消化，全程不需要
  人工放行（ADR-023 §4 生产缺省 `startup_autorun=true` 的形状）。
- 与幽灵水位用例（`live_startup_sweep_repairs_a_caught_up_ghost_watermark`，
  2026-08-14 证据）的分界：那条留着撒谎的登记行，这条连登记行都没有。

## 用例

`data_interface::increment_manager::tests::live_startup_sweep_baselines_a_never_parsed_db`

夹具手法：

- `delete_dbnum_fast`（DropRow）把 pe / 派生 / 统计 / 水位行全删，断言
  `DbnumState::read == None`（真「从未登记」）。
- watcher 换成只含目标库（7998）副本的一次性临时目录，收窄清单到单相位；
  基线解析按 `included_db_files` 文件名在项目目录定位正本，副本与正本同字节。
- `arm_auto_work()` 模拟生产缺省上弦（testbed 配置显式 `startup_autorun=false`），
  断言重扫行 `queued` 而非 `held`。
- 结尾以正本路径补一次 `scan_and_check_file`，`PathMigrated` 自动迁移还原登记
  路径（否则登记行指着即删的临时目录，下一轮全目录扫描判 Missing）。

## 结果

```powershell
$env:DB_OPTION_FILE='python/testbed/DbOption-pytest'
$env:AIOS_MANUAL_UPDATE_PROJECT='AvevaMarineSample'
$env:AIOS_MANUAL_UPDATE_DBNUM='7998'
$env:RUST_MIN_STACK='16777216'
cargo test --lib --features http_api data_interface::increment_manager::tests::live_startup_sweep_baselines_a_never_parsed_db -- --ignored --exact --nocapture
```

exit 0；1 passed；测试体 10.0s（2026-08-17 18:09，8019 testbed）。关键字面输出：

- `发现从未解析过的文件: ams7998_0001, db_type=DESI, 文件最新sesno: 12（入队后由基线接管）`
- `[live-startup-never-parsed] dbnum=7998 新排：sesno 1..=12（task db-20260817-180926-000000，排在第 1 位）`（无「挂起待增量触发」后缀）
- `dbnum=7998 基线已建立，排入 1 个全量生成根（等待模型任务消费）`
- `数据批次执行完毕 dbnum=7998 sesno 0..=12（…状态 succeeded…）`，任务回执含
  `首次按需初始化完成`
- 终态断言：`applied_sesno == 12`、`dbnum_has_any_pe_row == true`
- `F6 文件路径迁移 dbnum=7998: <临时目录> -> <正本>（已更新登记路径）`

## 首轮红跑的行为确认（有价值的副产品）

首版夹具直接对全部监控目录重扫：清单含沙箱 50+ 未解析库（三项目），
`drain_queue_until_empty` 只消化了 Meta 相位第一个批次（zdj7032 DICT）就按
ADR-025 的相位屏障停下——相位切换要求重建权威文件快照（`needs_rescan`），
这个重扫循环在生产 worker 的空闲轮里，不在 drain 里。结论：**多相位清单的
推进依赖生产 worker 的「相位重扫」循环**；测试要钉单库路由就必须收窄清单，
这与 2026-08-17「7998 消失」现场（Enqueue 追踪补 `phase_admits_dispatch`）
互为印证。
