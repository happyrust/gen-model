# `aios-database rvm-check`：一条命令跑完一个库 / 整个 MDB 的 RVM 对拍

给库号与 E3D 连接信息，自动枚举根、用 E3D TTY 导基准、生成、比对、汇总。页面上单根的
`?page=rvm-check` 链路的批量版，与它共用同一套生成、导出宏、比对与判色代码
（`src/rvm_check/`），所以结论口径一致。取代此前手工维护的 `_sites.txt` + `gen_ams.exe` +
`rvm_compare.exe` + `Invoke-SiteGenCompare.ps1` 那一串。

```powershell
# 在部署目录或仓库根下（与服务一样按工作目录读 DbOption.toml）
$env:RVM_CHECK_PASSWORD = '<E3D 密码>'
aios-database rvm-check --dbnum 7999,7326,7327,7331,7332,7352,7354,7356,7333 `
    --install-dir E:\reverse\e3d\shadow_e3d31_gen_model_test `
    --out output\rvm-check-batch\20261107-full
```

退出码：全部根 `pass` / `empty` 为 0，否则 1。明细在 `<out>/summary.json`（机器读）与
`<out>/summary.txt`（人读），每个根一份 `<out>/<tag>/<w0>_<w1>/run.json` + `compare.json`。

## 它做什么

| 步 | 做法 | 来源 |
|---|---|---|
| 枚举根 | 该库 WORLD 的直接成员里 noun 为 `SITE` / `APPLDW` 的元素（`--site-only` 只取 SITE）。两类根件集合互不重叠，合起来才是整库 | `DirectTreeService::children(world)` |
| 导基准 | **每库一次** `des.exe -tty` 会话，宏里逐根 `export`（`onerror continue`，每根一个哨兵）；哪个根没落出文件再单独按 refno / 名称补导一次 | `rvm_check::export_macro::export_batch_body` |
| 生成 | 缺省每个根起一个 `aios-database rvm-check --child` 子进程，父进程每 0.5 s 看一次可提交内存余量与耗时，越线就杀：`commit-guard` / `timeout` 只记在那个根上 | `rvm_check::generate_ours`（`e3d_model::rvm_export`） |
| 比对 | 进程内 `compare_rvm_files`（只读映射 + 并行，几百 MB 以内）；判色与页面同一函数 | `rvm_check::compare_rvms` / `judge_report` |

导出与生成是流水线：E3D 会话在自己的线程里逐库跑，生成工人不等它；一个根生成完、基准也到了
才比对。`--jobs N` 同时生成 N 个根（注意 ams7333 一个根要 9 GB）。

## 参数

| 参数 | 说明 |
|---|---|
| `--dbnum N[,N…]` / `--all-desi` | 要跑的 DESI 库；`--all-desi` 取当前 MDB 全部 DESI 库 |
| `--only REF[,REF…]` | 只跑这些根（`=w0/w1`、`w0/w1`、`w0_w1` 都认） |
| `--site-only` | 只取 SITE 类型的根 |
| `--out DIR` | 产物目录（缺省 `output/rvm-check-batch/<时间戳>`）；非空目录要 `--resume` 或 `--force` |
| `--resume` | 已比对出结果（`pass` / `empty` / `kind_differ` / `fail`）的根跳过 |
| `--baseline-dir DIR` | 不碰 E3D，从目录取 `<tag>_<w0>_<w1>.rvm`（如 `ams7999_24383_66456.rvm`） |
| `--stage full\|export\|generate\|compare` | 只跑某一步 |
| `--in-process` | 生成在本进程里跑（省一次库打开，没有内存隔离） |
| `--jobs N` | 同时生成的根数（缺省 1） |
| `--min-commit-free-gb` | 可提交内存余量低于此值就杀生成子进程（缺省 1.5） |
| `--gen-timeout-secs` / `--e3d-timeout-secs` | 单根生成 / 单次 E3D 会话超时（缺省 1200 / 3600） |
| `--install-dir` | `des.exe` 所在目录，要能跑 `-tty`（默认的 e3d31-runtime 在 TTY 下会崩） |
| `--projects-dir` / `--project-name` / `--e3d-project` / `--mdb` | 缺省取 `DbOption.toml`（`project_path` / `project_name` / 目录扫出的项目码 / `mdb_name`） |
| `--username` / `--password` | E3D 账号；密码不给时读 `RVM_CHECK_PASSWORD`。只进登录串，不进任何落盘文件 |

启动脚本按 `DbOption.toml` 的 `[rvm_check] e3d_launcher` 找，缺省是可执行文件同目录或当前目录下的
`scripts/e3d/run_ams_c_entrymacro.bat`。

## 根的终态

`pass` `empty`（两侧都没有可比几何，如 `/1AXIS`）`kind_differ` `fail` `no-baseline` `gen-failed`
`compare-failed` `commit-guard` `timeout`；`--stage export` / `generate` 单步成功记 `exported` /
`generated`。只有 `pass` / `empty`（单步时 `exported` / `generated`）算干净。

## 2026-11-07 实测（AMS 九库 19 根，本机 32 核 / 64 GB）

全部 `pass`（`/1AXIS` 为 `empty`）：both 243,522 / exact 243,522，off、only_*、unkeyable、
kind_differ 全 0。E3D 导出合计 441 s（每库一次会话，ams7333 两根 211 s），生成 175 s（ams7333
两根各 9.1 GB 峰值），比对 8.9 s；整批 10 分钟出头，产物 5.0 GB。
