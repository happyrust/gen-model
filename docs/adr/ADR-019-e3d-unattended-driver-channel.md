# ADR-019: E3D 无人值守驱动通道

- 状态：Accepted
- 日期：2026-08-06

## 决策

L3 runner 使用 `AVEVA_DESIGN_ENTRYMACRO`，由
`scripts/e3d/run_ams_c_entrymacro.bat` 启动独立 `/ALL` DESIGN 会话。runner 为每次调用生成
一层 wrapper 宏：写 `L3-ALIVE`、调用场景宏、写 `L3-DONE`、`QUIT`。成功判据是 launcher
退出码为 0、`L3-DONE` 存在且 `des.exe` 已退出；20 分钟超时会清理 `des.exe` 与
`pdmsconsole.exe`。

## 通道记录

| 通道 | 结果 | 证据 |
|---|---|---|
| TTY 命令行直带宏 | 未定标 | 保留为后续冷启动优化，不阻塞套件 |
| `PDMS_NOCONSOLE=1` + stdin | 失败 | stdin 不被命令循环消费；`run_ams_c_entrymacro.bat` 头注记录了标准句柄被 `pdmsconsole.exe` 接管 |
| ENTRYMACRO | 采用 | 既有增量宏与 `QUIT` 通道已跑通；套件哨兵为 `scripts/e3d/spike_sentinel.mac` |

## 后果

每场景使用新 E3D 会话，速度较慢，但启动、完成和超时清理都可观测。TTY 直带宏若以后同时
满足三段哨兵、60 秒退出和可采退出码，可整体替换 driver，场景表与判据无需修改。
