# 管道模型增量更新与 ATTA 模型验证（2026-09-04）

## 口径

- 候选、属性和会话差分只来自 e3d-io 对 E3D 3.1 二进制库的读取，使用
  `vendor/e3d-io/schema/e3d31/binaries/attlib.dat` 与同目录模板。
- 增量考生是 `update_ams`，裁判是同一 target 会话的 `gen_ams` 全量结果；没有使用旧 parser。
- 模型正确性只与 E3D 导出的 `test_data/rvm/1RCV0101-B1.rvm` 对拍。该文件由
  `scripts/e3d/rvm_export_1rcv0101_b1.mac` 只读导出，日志含 `ALIVE` / `DONE`，
  SHA-256 为 `4d25e1329bd6d96f9bef4c0d5bbaf39c85d47594e74e31d1c46fdc3543967843`。

## 修复

首次对拍发现 ATTA `24383/73951`、`24383/73956` 的表面位置接近 E3D，
但 E3D RVM 为 2 个连通体，生成模型错误合并成 1 个。`src/catalogue.rs` 现在对
ATTA 的可见目录图元保留独立部件；其他 noun 仍沿用原 CLFL 全程序判据。

原始版本由 e3d-model Git `d440af36acdb4d511be2a773886284b6e860cb5e`
及 `src/catalogue.rs` blob `996ce48` 固定。补丁是 `catalogue-atta-components.patch`，
回滚入口是 `rollback.ps1`。

## 四个真实物理窗口

命令模板：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\verify-window.ps1 `
  -BaseSesno S -TargetSesno T -Label LABEL
```

| 窗口 | E3D 操作 | full S -> T | 全量变化/消失 | upsert/remove | 不一致 | 结果 |
|---|---|---:|---:|---:|---:|---|
| 185 -> 186 | VALV 上移 500 mm | 10 -> 9 | 1 / 1 | 4 / 4 | 0 | PASS |
| 186 -> 187 | VALV 恢复 | 9 -> 10 | 2 / 0 | 5 / 3 | 0 | PASS |
| 189 -> 190 | BRAN HPOS U900 -> U1400 | 10 -> 10 | 0 / 0 | 3 / 3 | 0 | PASS |
| 190 -> 191 | BRAN HPOS 恢复 U900 | 10 -> 10 | 0 / 0 | 3 / 3 | 0 | PASS |

四次 `update_ams`、base `gen_ams`、target `gen_ams` 均为退出码 0；每窗的
`verification.json` 同时断言：全量变化无漏 upsert、upsert 与 target 完全相同、
消失项无漏 remove、remove 不指向 target 活项、无意外生成失败。

`189 -> 190` 的 e3d-io 属性差分明确为 BRAN `HPOS [3511.13,-8649.69,900] ->
[3511.13,-8649.69,1400]`。此变更只移动未成体的 HEAD 路由端点，当前三个可成管槽位
几何不变；增量保守重写三根管身，值与 target 全量一致。

已知数据边界：两端全量均报告 `24383/73952`、`24383/73953` 的同一悬空目录引用：
db7600 会话 12 不含 `23984/105633`。验证器只允许这两个既有失败，其他失败会使窗口失败。

## E3D RVM 验收

执行 `rvm_generated_obj_compare`，参数为 `--samples 12000 --max-surface-mm 5
--max-aabb-mm 0.5 --max-volume-percent 10`，两次退出码均为 0：

| 元素 | 双向 mean/p95/max (mm) | 拓扑 | AABB 最大差 | 体积差 | 结果 |
|---|---|---|---:|---:|---|
| 24383/73951 `/R342.062` | 0.124644 / 0.294926 / 0.365133 | 2 = 2 | 0.239258 mm | 3.758793% | PASS |
| 24383/73956 `/R342.009` | 0.124727 / 0.294985 / 0.365422 | 2 = 2 | 0.244873 mm | 3.763316% | PASS |

完整字面输出在 `rvm-73951.stdout.txt`、`rvm-73956.stdout.txt`。

## 测试记录

- `cargo test --offline atta_mixed_clfl_program_keeps_independent_components --lib`：
  1 passed，退出码 0。
- `GATE_ONLY=ams8000_0001:249:250 cargo test --offline --test increment_real
  an_increment_says_what_two_full_generations_say -- --exact`：1 passed，128.78 s，退出码 0。
- `cargo test --offline --test increment_real
  hvac_owned_branches_derive_no_implied_tubes_but_pipe_branches_still_do -- --exact`：
  1 passed，退出码 0。
- `cargo test --offline --test increment_real
  the_affected_closure_reaches_both_owners_of_a_reparented_element -- --exact`：
  1 passed，退出码 0。
- `cargo fmt --all -- --check`：退出码 1，仅命中本次范围外已有的
  `src/elmodl.rs`、`src/section.rs` 格式差异；本次 `src/catalogue.rs` 无格式差异。
