# Implementation Plan：启动修复无数据支撑的应用水位

## Constitution Check

- 水位承诺：空基线凭据与水位同事务，失败不推进。
- 单一规则：幽灵水位与空基线判据由共享纯函数承载。
- 响亮失败：状态/数据支撑读取失败进入日志或回执。
- 单队列：自动与手动仍走 ADR-011 的同一队列和 worker。
- 可执行守护：纯函数、SQL 渲染、启动入队与 live 回归分别钉住。

## Changes

1. `src/data_interface/dbnum_state.rs`：读取并暴露 `confirmed_empty_baseline_sesno`。
2. `src/data_interface/model_update_pending.rs`：基线尾事务写入/清除空基线凭据。
3. `src/data_interface/manual_update.rs`：共享幽灵水位判据，预览/手动/执行统一。
4. `src/data_interface/increment_manager.rs`：启动重扫检出追平幽灵水位并构造首次导入批次。
5. `src/data_interface/batch_worker.rs`：冻结点预判读取空基线凭据。
6. `src/data_interface/fast_delete.rs`：重建清水位时同步清除凭据。
7. `src/options.rs`、`DbOption.toml`：生产默认启动自动执行。
8. 更新 `CONTEXT.md`、配置台账、changelog 与 live 证据。

## Verification

- 定向 Rust 单测（CI feature 口径）。
- `cargo fmt`、`cargo check`。
- pytest 沙箱 live：追平幽灵水位经启动扫描恢复；同步更新 live 台账。
