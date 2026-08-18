# 逐会话回放跨仓编译隔离与 protoc 退役证据（2026-08-19）

## 结论

- 默认/正式 release 的 `aios-database` 生产 feature 集不启用
  `legacy_session_replay`；`IncrementPipeline::collect_changes` 与 vendor 回放 API 在
  类型层面缺席，净窗口唯一入口仍是 `collect_window → collect_net_window`。
- `aios-py`、两个主仓诊断 bin、六个 vendor 诊断 bin 与两个 replay oracle 目标显式
  启用该 feature，独立参照臂仍可用。
- `dpcsync` 不再运行 `prost-build`：删除 `build.rs`，检入原生成的
  `src/chunk_dictionary.rs`（SHA-256
  `f1923a074924f879ef40d154d8f49976aaa6300e86d70b0576b12b67904ed1b7`）。
  全部验证均在 `PROTOC` / `PROTOC_INCLUDE` 未设置时运行。

## 跨仓版本

| 仓库 | 分支 / 提交 | 说明 |
|---|---|---|
| `dpc-sync` | `codex/remove-protoc-build` / `d7ce7fd848a138e5fc3ebd88ef55da00ee0ac780` | 删除宿主 protoc 构建依赖，已推送并以 `ls-remote` 复核 |
| `old-pdms-io` | `codex/room-panel-wire-repair-deps` / `41744e7` | T14 共享 `diff_ele_data` 独立提交 |
| `old-pdms-io` | 同上 / `22476169342b0d684cf8445146e0ea39e30a6c47` | 回放 feature 隔离并钉住 dpcsync，已推送并以 `ls-remote` 复核 |

主仓 `Cargo.toml` 与 `python/Cargo.toml` 均钉住 vendor 完整 SHA；`Cargo.lock` 的
`dpcsync` source 为 `d7ce7fd8…`，依赖列表只含运行期 `prost`，没有 `prost-build`。

## 编译与纯测试记录

| 命令（均 exit 0，除“缺席证明”所列预期值） | 字面结果 |
|---|---|
| dpcsync `cargo check` | 0 errors；4 条既有 warning |
| dpcsync `cargo test --lib` | 47 passed |
| dpcsync `cargo tree -i prost-build` | exit 101，`package ID specification prost-build did not match any packages`（预期缺席） |
| vendor `cargo check --lib --no-default-features` | 0 errors |
| vendor `cargo test --doc --no-default-features` | 3 passed（含 compile-fail） |
| vendor feature 正向类型测试 | 1 passed |
| vendor `element_diff_tests` | 2 passed |
| 主仓正式 release build（`ws,gen_model,manifold,occ,project_hd,http_api`） | 0 errors，201 个既有 warning；feature tree 中 `legacy_session_replay` 命中 0 |
| 主仓无 feature 生产 `cargo check` | 0 errors，201 个既有 warning |
| 主仓无 feature compile-fail doctest | 1 passed |
| 主仓无 feature pipeline / net-window | 47 passed, 2 ignored / 13 passed |
| 主仓有 feature API 正向类型检查 | 1 passed |
| `db8000_two_delete_fixture` / `db8000_session_pairs`（有 feature） | 6 passed / 20 passed |
| Python `maturin develop` + offline | 构建成功；84 passed, 23 deselected |

首次在长路径隔离 target 下构建 `manifold-csg-sys` 因 Windows 260 字符限制失败；改用
短 target `D:\ct\lrbi` 后同一命令通过。没有清理或改写共享 target。

## live 与红证

两条纯文件对拍显式启用 `legacy_session_replay`：

- `live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay`：1 passed，21.21s；
- `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`：1 passed，23.11s。

issue-019 使用仅含 `amssys + ams8000` 的运行目录副本。现场 E3D 正占用 testbed
正本，因此通过 `AIOS_ROOMTEST_CONFIG` / `AIOS_NET_AB_DB_FILE` 指向隔离副本；生产代码、
数据库 DTO 与夹具真值未改。副本起始/恢复 SHA 都是 final@26 的
`84b0040fdbc242d406540eab3d511d41a44aac899f55106821a93f5e419e6454`。

1. 正常合跑：`2 passed in 37.52s`，exit 0；字面签名
   `changed=3 sessions=[25,26] tombstones=['24384_24778','24384_24779'] watermark=26`。
2. `AIOS_T11B_FORCE_EMPTYRUN=1`：`1 error in 32.20s`，pytest exit 1；准确失败于
   `固定删除目标在起点不是活行`，实际活行集合为 `[]`。
3. 清除变量立即复跑：`1 passed in 37.43s`，exit 0；EQUI/BOX 起点活行、终态立碑。

## release 记录项

在隔离的 latest=230 文件副本上复跑
`live_ams8000_single_caliber_release_timing`（1 passed，exit 0，7.46s）：

- high-retouch `115..=230`：net ops 68，replay ops 236，复触率 3.47；warm
  median/min/p95 = `11/11/11ms` vs `60/59/60ms`，约 5.5×；
- add-floor `1..=230`：net ops 6545，replay ops 6943，复触率 1.06；warm
  median/min/p95 = `171/123/194ms` vs `1185/802/1215ms`，约 6.9×。

这是 ADR-031 记录项，不改变单路径决定；SYST 250206 仍为上线后现场复测。

## 可复核产物

本轮修改包、跨仓 patch、逐命令验证记录与回滚脚本位于：
`test-increment/runs/legacy-replay-build-isolation-20260819-001630/`。回滚脚本只在该目录
生成的独立沙箱中执行验证，不触碰三个工作仓。
