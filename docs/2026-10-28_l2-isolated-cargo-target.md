# L2 回放必须用独立 CARGO_TARGET_DIR：os error 5 陷阱与判绿口径（2026-10-28）

适用：`cargo test --test increment_replay`（L2 回放夹具，`tests/increment_replay/`）及调用它的 `scripts/mission-gate.ps1`。
唯一跑法入口：`scripts/replay/Run-L2Replay.ps1`。

## 1. 现象：编译期 os error 5，退出码与 L2 真红同为 101

用默认 target（本机为 `D:\Rust\target`）跑 L2 时，cargo 在重新链接被测二进制的那一步失败，报错形状：

```text
error: failed to remove file `D:\Rust\target\debug\aios-database.exe`

Caused by:
  拒绝访问。 (os error 5)
```

（`Caused by` 那一行是操作系统的本地化文案，英文系统为 `Access is denied. (os error 5)`；判定只认 `os error 5`。）

这条路径下 cargo 的退出码是 **101**，和 L2 用例断言失败（`#[test]` panic）的退出码**一模一样**。只看退出码的门禁或人会把
「环境没编译过」误读成「L2 红」，然后去产品代码里找不存在的回归。判定必须看日志，不能只看退出码。

## 2. 根因：常驻服务独占了默认 target 里的 debug 二进制

- 本机常驻的开发服务 `aios-database.exe`（监听 0.0.0.0:8022）就是从 `D:\Rust\target\debug\aios-database.exe` 起的。
- Windows 对正在运行的 exe 文件加锁，不允许删除 / 覆盖。
- `cargo test --test increment_replay` 要把被测二进制（`CARGO_BIN_EXE_aios-database`）和夹具二进制（`db_session_fixture`）都编出来；
  只要源码有变动需要重链接，就要先删掉旧的 `debug\aios-database.exe` → 拒绝访问 → `os error 5` → 退出 101。
- 所以这不是偶发：只要常驻服务在跑、且距上次编译后有任何改动，用共享 target 跑 L2 **必然**命中。

## 3. target 目录可用性实测

| 目录 | 状态 | 结论 |
| --- | --- | --- |
| `D:\Rust\target` | 存在、可编译，但 `debug\aios-database.exe` 被常驻服务独占 | **不能用**：重链接必然 os error 5，退出 101 与 L2 红无法区分 |
| `D:\Rust\target-l2-m1cwd` | 存在，L2 整档 1476 s 跑通 | **默认值**（`Run-L2Replay.ps1 -TargetDir` / `mission-gate.ps1 $L2TargetDir`） |
| `D:\Rust\target-l2-verify8` | 从零编译崩溃 `0xc0000409` | 不可用 |
| `D:\Rust\target-l2` | 不存在 | 旧的 `mission-gate.ps1` 默认值，已改掉 |

为什么不能随手新建一个 target：新目录 = 从零编译整棵依赖树，耗时长不说，`target-l2-verify8` 已经实测崩过（`0xc0000409`，
栈缓冲区 / 快速失败，多半是编译期内存 / 链接器问题，没有继续追）。要换目录先手工跑通一次全档，再把两个脚本的默认值一起改。
`Run-L2Replay.ps1` 对不存在的 `-TargetDir` 默认拒绝（退出 3），要试新目录须显式加 `-AllowNewTargetDir`。

`.gitignore` 已有 `/target-*`：仓内的隔离 target 不会进检出；`D:\Rust\target-*` 在仓外，更不会。

## 4. 正确跑法

```powershell
# 全档（约 25 min；默认 CARGO_TARGET_DIR=D:\Rust\target-l2-m1cwd，影子根不存在会自动造）
pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1

# 单例（墙钟下限传 0；给了 -Case 又没显式传 -MinWallClockSec 时脚本自动取 0）
pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -Case ams8000-equi-data-fail-park -MinWallClockSec 0

# 放行 known_red 用例
pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -IncludeKnownRed

# 只看将要执行的环境与命令，不跑、不落盘
pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -DryRun
```

脚本内部等价的手工命令（只在排障时手敲，平时走脚本）：

```powershell
$env:CARGO_TARGET_DIR = 'D:\Rust\target-l2-m1cwd'
$env:AIOS_REPLAY_PROJECT_ROOT = '<影子工程根>'      # scripts/replay/New-ShadowProjectRoot.ps1 -Target <路径> 造，须与 D:\AVEVA\Projects\E3D3.1 同卷
# $env:AIOS_REPLAY_CASE = '<用例名>'             # 可选：只跑一条
# $env:AIOS_REPLAY_INCLUDE_KNOWN_RED = '1'       # 可选：放行 known_red
cargo test --locked --test increment_replay --features ws,gen_model,manifold,project_hd -- --test-threads=1 --nocapture
```

要点：

- `--test-threads=1` 必须：整档是一个 `#[test]`，每条用例一套 surreal + 被测服务进程，不能并行。
- `--locked` 必须：与 CI 同口径，不让本地跑悄悄改 `Cargo.lock`。
- features 固定 `ws,gen_model,manifold,project_hd`（不带 `http_api`，与 CI 一致）。
- 脚本跑前记录既有 `aios-database` / `surreal` 进程清单到日志头，**只记录、绝不 kill**：常驻服务是别人的工作现场。
- 产物在 `_runs\l2-<Owner>\logs\<stamp>-l2.log`（完整 stdout+stderr）与同目录 `summary.txt`（每次追加一段 `key=value`：
  退出码、墙钟、判定等）。

## 5. 判绿三条（缺一不可）

1. **退出码 0**。
2. **整档墙钟 > 1000 s**（实测 1476 s）。低于这个数就没真跑；单例把下限传 0。
3. **日志里没有 `skip increment_replay:`**。这是 harness 前置不齐时的整档跳过（`tests/increment_replay/main.rs` 的 `environment()`
   返回 Err → `eprintln!("skip increment_replay: {reason}")` → `return`），整档 1 s 报 ok。典型原因：跨 worktree 复用 target，
   二进制里用 `env!("CARGO_MANIFEST_DIR")` 嵌死的仓根指向别的检出，仓根 `DbOption.toml` / `bin\surreal.exe` 找不到；或影子根下
   没有 `AvevaMarineSample`。

`Run-L2Replay.ps1` 把这三条和两类环境错误做成了自动判定，并用自己的退出码把它们从 cargo 的 101 里分出来：

| 脚本退出码 | 含义 | 判据 |
| --- | --- | --- |
| 0 | 真绿 | cargo 退出 0 + 墙钟 ≥ 下限 + 无 `skip increment_replay:` |
| 2 | 假绿 | 日志有 `skip increment_replay:`，或 cargo 退出 0 但墙钟低于下限 |
| 3 | 环境：target 被占用 | 日志有 `os error 5`（也用于 `-TargetDir` 不合法 / 造影子根失败） |
| 4 | 环境：抢仓根单实例锁 | 日志有 `stage=instance-lock`（M1 已修，不该再出现；出现就查 harness 给被测服务的 CWD） |
| 其它 | L2 真红或编译错误 | 原样透传 cargo 退出码（101），看日志里的 panic / 断言 |

## 6. 为什么独立 target 在并发 aios-database.exe 下仍能编译

`CARGO_TARGET_DIR` 决定 cargo 把产物写到哪。独立 target 下有**自己的一份** `debug\aios-database.exe` / `debug\db_session_fixture.exe`，
与常驻服务正在运行的那份（`D:\Rust\target\debug\aios-database.exe`）是两个文件：cargo 删的、写的都是自己这份，没人锁它；
常驻服务那份一直没被碰。L2 跑起来的被测服务（每条用例一套，随机回环端口、运行目录当 CWD）起的也是独立 target 里的那份，
与常驻服务并存：端口不同、CWD 不同（单实例锁 `.gen-model.instance.lock` 按 CWD 取，所以也不会抢仓根那把锁）、二进制文件不同。
同理，两个人各用各的 target 也能同时编译（`.gitignore` 里 `/target-*` 的注释就是这个用法：共享 target 还会给出 stale rlib）。

代价：独立 target 是整棵依赖树的另一份编译产物，首次编译慢、占盘。所以才要复用已实测可用的 `D:\Rust\target-l2-m1cwd`，而不是每次新建。

## 7. 另外两个坐过的坑（脚本已纳入判定）

- **假绿 `skip increment_replay:`**：见 §5 第 3 条。跑完 1 s 就 `test result: ok` 的，一律当没跑。
- **`stage=instance-lock`**：被测服务按进程 CWD 取单实例锁，仓根那把常被常驻服务持有；harness 若用仓根当 CWD，每条用例都在
  这一步退出 1（M1 已改成运行目录当 CWD，当前不该出现；出现就是回归）。
