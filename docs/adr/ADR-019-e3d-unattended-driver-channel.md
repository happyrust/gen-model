# ADR-019: E3D 无人值守驱动通道

- 状态：Accepted
- 日期：2026-08-06

## 决策

L3 runner 使用 `AVEVA_DESIGN_ENTRYMACRO` 投递 wrapper，并由
`scripts/e3d/run_ams_c_entrymacro.bat` 以 `des.exe -tty PROJECT LOGIN MDB` 启动无 UI DESIGN
会话。driver 显式绑定 `L3_PROJECT_WORK` 的父目录及该副本自己的项目 evars、自动登录并等待该次 `des.exe` 退出；runner
为每次调用生成 wrapper：写 `L3-ALIVE`、调用场景宏、写 `L3-DONE`、`QUIT`。

成功判据是本次 launcher 退出码为 0、`L3-ALIVE`/`L3-DONE` 均存在且场景日志可读。超时只按
本次 PID 树清理 `des.exe`/`pdmsconsole.exe`，不再按映像名误杀别的 E3D 会话。

## 通道记录

| 通道 | 结果 | 证据 |
|---|---|---|
| `-tty` + `AVEVA_DESIGN_ENTRYMACRO` | 采用 | 2026-08-06 只读哨兵实跑：冷启动至完整退出 2.99s、退出码 0，A/B/C 三段日志齐，本次 des/pdmsconsole 零残留；证据在 `output/e3d-spike/`、`output/e3d-tty-verification/` |
| TTY 位置参数直带宏 | 不采用 | Startup 参数解析会把 project/login/mdb 后的 token 当 macro；环境变量投递避免命令行引号歧义 |
| `PDMS_NOCONSOLE=1` + stdin | 失败 | stdin 不被命令循环消费；`run_ams_c_entrymacro.bat` 头注记录了标准句柄被 `pdmsconsole.exe` 接管 |
| 无 `-tty` 的 ENTRYMACRO | 旧通道 | 既有增量宏曾跑通；现由 `-tty` 无 UI 会话替代 |

## 后果

每场景使用新 E3D TTY 会话，速度较慢，但项目、登录、MDB、启动、完成、退出码与定向清理
都可观测。调用方可用 `L3_E3D_PROJECTS_DIR`、`L3_E3D_PROJECT`、`L3_E3D_LOGIN`、
`L3_E3D_MDB` 覆盖默认值；密码不写入日志。
