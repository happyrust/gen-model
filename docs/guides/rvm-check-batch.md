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

## 基准目录（`--baseline-dir`）的版本要求

目录里的 `<tag>_<w0>_<w1>.rvm` 必须是**同一份项目数据、同一 E3D 版本**导出的基准。拿旧版本
（如 E3D 3.1 时代）导出的基准去比 E3D 2.1 项目数据，差异不是系统性的：表现为少量构件的
`only_baseline` / `only_generated` / `off` / `kind_differ`（个位到数十个），逐根排查会误判成
模型缺陷（issue #116 即此类）。换项目数据或换 E3D 版本后，要用 `--stage export`（或 `full`）
重新导一批基准，并在目录里留一份来源说明（批次目录、schema 版本、日期）。

## 根的终态

`pass` `empty`（两侧都没有可比几何，如 `/1AXIS`）`kind_differ` `fail` `no-baseline` `gen-failed`
`compare-failed` `commit-guard` `timeout`；`--stage export` / `generate` 单步成功记 `exported` /
`generated`。只有 `pass` / `empty`（单步时 `exported` / `generated`）算干净。

## 结果入库：`rvm_check_run` / `rvm_check_mismatch`

跑完缺省把每个根写进 `DbOption.toml` 指的那台 SurrealDB（`src/rvm_check/store.rs`；页面上的单根比对
也写同两张表）。`--no-store` 关掉；`store_mode` 为 mem 时没有可写的持久库，自动跳过并说明。
表是 SCHEMALESS 的，首次用到时建表建索引。

| 表 | 一行是什么 | 关键字段 |
|---|---|---|
| `rvm_check_run` | 一次比对（一个根） | `run_id`（页面是任务号；批量是 `<batch_id>/<tag>/<w0_w1>`，`batch_id` = 产物目录最后两级）、`source`（page / batch）、`refno` `dbnum` `noun` `name`、`verdict`（pass / kind_differ / fail）、`status`（更细的终态）、`stage` `message`、`counts`、`mismatch_total` `mismatch_by_kind` `mismatch_by_category`、`seconds` `gen_peak_private_mb`、`gen_ledger`（生成侧 `report.json` 的账本计数：skipped / evidence_pending / centreline_only / unknown_nouns …）、`artifact_dir` `files`、`finished_at` `finished_unix_ms` |
| `rvm_check_mismatch` | 一条差异 | `run_id` `refno` `dbnum` `seq`、`category`（off / kind_differ / only_baseline / only_generated / unkeyable_* / container_* / `stage_failure` / `centreline_only`）、**`reason_kind`**（原因种类）、**`reason`**（带数值的一句话）、`severity`（error / warning / info）、`name` `noun` `max_axis_delta_mm` `baseline_kinds` `generated_kinds` `baseline_aabb` `generated_aabb` |

`reason_kind` 是比 `category` 细一档的归因（`rvm_check::report::reason`）：

| 原因种类 | 含义 |
|---|---|
| `aabb_shifted` | 尺寸一致、位置整体偏移（reason 里给出三轴偏移量） |
| `aabb_axes_swapped` | 两根轴长互换——局部标架转了 90° |
| `aabb_one_axis_size` | 只有一根轴长度不同（端面偏移、长度算错一类） |
| `generated_larger` / `generated_smaller` | 生成体整体包住基准 / 被基准包住（多出 / 缺一块） |
| `aabb_off` | 以上都不是的一般超差 |
| `container_tube_mismatch` | 包围盒在容差内，是容器组内隐式管逐根账把它判成 off 的 |
| `facet_vs_analytic` / `primitive_count_differs` / `primitive_kind_differs` | 图元种类：面片对解析图元 / 条数不同 / 种类不同 |
| `missing_implied_tube` / `missing_named_element` / `missing_unnamed_element` | 只在基准（E3D 有、我们没有） |
| `extra_implied_tube` / `extra_in_generated` | 只在生成 |
| `unkeyable_baseline` / `unkeyable_generated` | 发不出主键（reason 抄的是比对器给的括号说明） |
| `container_tube_off` / `container_tube_missing_in_generated` / `container_tube_extra_in_generated` | 容器组内逐根账 |
| `centreline_only` | E3D 只画中心线（`info`，不进判据，只记账） |
| `generate_timeout` / `generate_commit_guard` / `generate_panic` / `generate_unbalanced` / `e3d_export_no_file` / `e3d_launcher_missing` / `compare_panic` / `baseline_missing` … | 某一步没跑完（`category = stage_failure`，由阶段与报错文本认） |

服务端查询（都是 GET，参数即过滤条件，不给就不限）：

```text
/api/v1/rvm-check/history?refno=24383/73927&dbnum=7999&verdict=fail&batch_id=…&before=<finished_unix_ms>&limit=50
/api/v1/rvm-check/mismatches?run_id=…&refno=…&dbnum=…&category=off&reason_kind=aabb_shifted&noun=CTRAY&severity=error&limit=500&offset=0
/api/v1/rvm-check/reasons?dbnum=7022            → 按 reason_kind / category / severity 的计数，多的在前
```

直接查库的写法一样：

```sql
SELECT reason_kind, noun, count() AS n FROM rvm_check_mismatch WHERE dbnum = 7022 AND severity = 'error' GROUP BY reason_kind, noun;
SELECT run_id, verdict, mismatch_by_kind FROM rvm_check_run WHERE refno = '23406/1' ORDER BY finished_unix_ms DESC;
```

## 2026-11-07 实测（AMS 九库 19 根，本机 32 核 / 64 GB）

全部 `pass`（`/1AXIS` 为 `empty`）：both 243,522 / exact 243,522，off、only_*、unkeyable、
kind_differ 全 0。E3D 导出合计 441 s（每库一次会话，ams7333 两根 211 s），生成 175 s（ams7333
两根各 9.1 GB 峰值），比对 8.9 s；整批 10 分钟出头，产物 5.0 GB。

## 2026-11-08 实测（AMS E3D 2.1 数据，14 根）

腾讯云 Win2012R2（4 vCPU / 8 GB + 16 GB pagefile）部署验证时发现 3 个根报 `fail`，排查结论是
**基准版本错配而非模型缺陷**：随包基准是 E3D 3.1 时代导出的，与 E3D 2.1 项目数据存在少量构件
差异。用 `e3d_version = "e3d210"` 重新导出基准后全量复验：13 `pass` + 1 `empty`，off / only_* /
kind_differ 全 0；三个原 fail 根（`ams8000 =24384/24932`、`=24384/22399`、`ams7999 =24383/66456`）
分别 129 / 1,982 / 4,118 个构件全部 exact。ams7333 两根生成峰值约 9.0 GB，8 GB 内存机器必须
保留 ≥16 GB pagefile 并 `--jobs 1` 串行；可提交内存余量不足时 `--min-commit-free-gb` 会把
单个根记成 `commit-guard`，属资源保护而非比对失败。
