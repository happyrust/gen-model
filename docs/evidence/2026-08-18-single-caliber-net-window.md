# 2026-08-18 单一净窗口口径：release 完整收集计时（T18 记录项）

状态：**记录项，非门**（[ADR-031](../adr/ADR-031-single-net-window-collection-caliber.md)）。

> 2026-08-19 P5 编译隔离后复跑：显式启用 `legacy_session_replay`，latest=230，
> high-retouch warm median `11ms vs 60ms`（约 5.5×），add-floor
> `171ms vs 1185ms`（约 6.9×），1 passed / exit 0。完整命令与三仓版本见
> [编译隔离证据](2026-08-19-legacy-session-replay-build-isolation.md)。
数字如实记，不据此决定走哪条收集路径——生产已经只有净窗口。

SYST `250206` 单趟 < 30s **未测**（该库在客户现场）。本地 amssys / testbed 8000
只是代理形态。复测不达标的处置是 `git revert` 单路径提交。

## 协议

- 构建：`cargo test --release --locked --lib … --no-default-features --features ws,gen_model,manifold,project_hd,legacy_session_replay`
- 用例：`live_ams8000_single_caliber_release_timing`（`net_window.rs` tests，`#[ignore]`）
- 计时对象：生产入口 `IncrementPipeline::collect_window`（打开文件 + 会话页清单 +
  `collect_net_window`）对 legacy `collect_changes`。**不是**已打开 `PdmsIO` 上的
  内层 `collect_net_window`——T18a 的 3ms / 17.7× 是内层单点，和本表不可直接比。
- 每窗：1 次 cold（进程内第一次，作 warmup 并另报）+ 5 次 warm；报 median / min / p95。
- 复触率 = 回放非 `None` 操作数 ÷ 净窗口操作数。

## 环境

| 项 | 值 |
|---|---|
| 日期 | 2026-08-18 |
| OS | Windows 10.0.26200 |
| CPU | AMD Ryzen 9 7950X 16-Core |
| RAM | 64 GiB |
| rustc | 1.99.0-nightly (`1a98b1e13` 2026-08-07) |
| git HEAD | `bdb5d180`（工作区含未提交的 ADR-031 切换） |
| 文件 | `python/testbed/projects/AvevaMarineSample/ams000/ams8000_0001` |
| 大小 | 16,504,832 字节 |
| SHA256 | `6499852d7934ca087fdf4eb28e00767a724fef0fb21b1f90dc5290244dcae9a9` |
| latest sesno | 209 |
| cfg | release（`debug_assertions` off） |

## 数字

| 窗口 | 会话 | net_ops | replay_ops | 复触率 | cold net / replay | warm net median/min/p95 | warm replay median/min/p95 | 倍数（median） |
|---|---:|---:|---:|---:|---|---|---|---:|
| **high-retouch** `104..=209` | 106 | 66 | 212 | **3.21** | 28ms / 55ms | **10 / 9 / 10 ms** | **53 / 53 / 53 ms** | **≈5.3×** |
| add-floor `1..=209` | 209 | 6546 | 6899 | 1.05 | 116ms / 780ms | 128 / 123 / 180 ms | 908 / 806 / 1030 ms | ≈7.1× |

原协议跑时 6.51s（含两次 cold + 各 5 次 warm）。本轮修复后复跑同一 release
记录项：`1 passed`，用例计时 6.05s，exit 0；收集结果未改变。

## 怎么读

- 高复触窗是净收集的**动机形状**。本轮生产入口（含打开文件）warm median 5.3×，
  低于 T18a 内层单点 17.7×，也低于原验收 4 的 ≥10×——**如实记，不作门**。打开文件
  的固定成本把短窗的倍数压下去了；净路径 warm 10ms 对回放 53ms，绝对值已经不是
  决策变量。
- Add 地板窗复触率 1.05，本就不该快多少。7.1× 是形态决定的，不能拿来判定门。
- 与 debug 全窗 8.8×、纯差分 15–34×、A/B probe 混层 4.4× 的对照关系不变：
  后两者不是「完整收集对完整收集」。

## 关联回归

- issue-019 固定窗口全链签名与 T11b 正常合跑：`2 passed in 32.94s`，exit 0。
- `AIOS_T11B_FORCE_EMPTYRUN=1`：固定目标起点活行断言按预期报错，exit 1；清除变量后
  立即复跑 `1 passed in 32.68s`，exit 0。
- 两条纯文件 live 对拍本轮复跑均通过：15.83s / 18.37s，收集结果未改变。
- 详细签名、红证与原文件恢复 SHA 见
  `2026-08-18-net-window-stable-signature-live.md`。

## 未测

- SYST `250206` 现场硬门（上线后复测项）。
