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
| `--only REF[,REF…]` | 只跑这些根（`=w0/w1`、`w0/w1`、`w0_w1` 都认）；`--unit zone` 时可以点名单元，也可以点名上级 SITE / APPLDW（选中它下面的全部单元） |
| `--site-only` | 只取 SITE 类型的根 |
| `--unit root\|zone` | 比对单元。缺省 `root`（一个 SITE / APPLDW 一个单元）。`zone`：根下的每个 ZONE 各成一个单元（APPLDW 下经 APPLDA、APPDAR 两层才到 ZONE；不在任何 ZONE 里的子节点各成一个兜底单元），各自导基准、各自生成即比对、不写 `generated.rvm`；只支持 `--stage full` / `export` |
| `--list-units` | 只列出比对单元（写 `<out>/units.json`），不碰 E3D、不生成 |
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

## 结果入库：`rvm_check_run` / `rvm_check_mismatch` / `rvm_check_element`

缺省把结果写进 `DbOption.toml` 指的那台 SurrealDB（`src/rvm_check/store.rs`；页面上的单根比对
也写同三张表）。**一个根（`--unit zone` 时一个单元）跑完就写，不等整批结束**——长批次中途断了，
跑完的那些已经在库里。`--resume` 续跑时，跳过的根 / 单元里库里还没有的会补进去（上次加了 `--no-store`、
当时库连不上、或中途断了），已经在库里的不动。`--no-store` 关掉；库连不上或 `store_mode` 为 mem
时告警后照跑，产物目录里的 JSON 仍是完整的一份。表是 SCHEMALESS 的，首次用到时建表建索引。

前两张表到单元为止；第三张 `rvm_check_element` 到构件：一个带几何的构件一行，**通过的也有**，记录 id
由 refno 得出（见下面「构件级结果」）。

| 表 | 一行是什么 | 关键字段 |
|---|---|---|
| `rvm_check_run` | 一次比对（一个根 / 单元） | `run_id`（页面是任务号；批量是 `<batch_id>/<tag>/<w0_w1>`，`batch_id` = 产物目录最后两级）、`source`（page / batch）、`refno` `dbnum` `noun` `name`、`unit`（`root` / `zone` / `fallback`）、`root_refno` `root_noun` `root_name`（所在的上级根：单元行是它的 SITE / APPLDW，根行是它自己）、`verdict`（pass / kind_differ / fail）、`status`（更细的终态）、`stage`（停在哪一步：`resolve` / `generate_ours` / `e3d_export` / `compare`，页面与批量同一套四个名字）、`message`、`counts`、`mismatch_total` `mismatch_by_kind` `mismatch_by_category`、`seconds` `gen_peak_private_mb`、`gen_ledger`（生成侧 `report.json` 的账本计数：skipped / evidence_pending / centreline_only / unknown_nouns …）、`artifact_dir` `files`、`finished_at` `finished_unix_ms` |
| `rvm_check_mismatch` | 一条差异 | `run_id` `refno`（所在的根 / 单元）`root_refno` `dbnum` `seq`、`category`（off / kind_differ / only_baseline / only_generated / unkeyable_* / container_* / `stage_failure` / `centreline_only`）、**`reason_kind`**（原因种类）、**`reason`**（带数值的一句话）、`severity`（error / warning / info）、`name` `noun` `max_axis_delta_mm` `baseline_kinds` `generated_kinds` `baseline_aabb` `generated_aabb` |

`unit` 与 `root_*` 是 2026-11-10 加的；页面单根，以及此前写进来的批量行没有这几格。按上级根查的时候，
没有 `root_refno` 的行把自己的 `refno` 当上级根（那时一行就是一个根），所以老记录照样查得到。

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
| `generate_timeout` / `generate_commit_guard` / `generate_panic` / `generate_unbalanced` / `e3d_export_no_file` / `e3d_launcher_missing` / `baseline_missing` … | 某一步没跑完（`category = stage_failure`，由阶段与报错文本认） |
| `compare_panic` / `compare_duplicate_key` / `compare_duplicate_name` / `compare_unknown_bbox_frame` / `compare_failed` | 比对这一步没跑完（同是 `stage_failure`）：比对时 panic / 主键重复 / E3D 名称重复 / bbox 帧认不出来 / 其他。批量的终态是 `compare-failed` |
| `compare_nothing_paired` | 比对跑完了、判红，却一件都没配上（两侧的主键口径对不上） |
| `run_failed` | 兜底：阶段认不出来。正常不会出现 |

比对这一步的阶段名只有 `compare` 一种写法。停在这一步的有两种情形，靠终态分：`fail` 是比对跑完判红，差异在
清单里；`compare-failed` 是比对没跑完，留一行 `stage_failure`，原因种类按报错文本分成上表那五种。
（2026-11-10 之前批量侧把后一种的阶段写成 `compare_failed`，归类认不出，原因种类全落到 `run_failed`。）

### 构件级结果：`rvm_check_element`

一个节点（一个 ZONE 单元、一个整根，或 `--scope` / 页面上指定的某个节点）核对完，它下面**每个带几何的构件**
在这张表里各有一行，通过的也有（2026-11-10 加）。批量命令（`--unit zone` / `--unit root`、`--only`、
`--scope node:` / `subtree:`，走不走生成即比对都一样）与页面单根都写。

**记录 id 由 refno 得出**，所以按 refno 直接取得到：

| 构件 | 记录 id | 备注 |
|---|---|---|
| 有自己 refno 的 | `rvm_check_element:⟨24383_66460⟩`（`<w0>_<w1>`，与 `pe` 的键同一写法） | `refno = '24383/66460'` |
| 隐式管身（库里没有记录，由属主 BRAN 派生） | `rvm_check_element:⟨24383_66459:TUBI:1⟩`（属主 refno + 管号） | `refno` 是属主的；`derived = true`、`derived_kind = 'implied_tube'`、`derived_ordinal`（管号）、`tube_from` / `tube_to`（两端的构件）。只在基准侧的那根用 E3D 的管号，键是 `…:TUBI:E<n>` |
| 吊架的轴线管（HANG 自己没有几何，E3D 给它画一根 `TUBI 1 of HANG /H`） | `rvm_check_element:⟨23717_113795:TUBI:1⟩`（HANG 的 refno + 1） | `refno` 是 HANG 的；`derived = true`、`derived_kind = 'hang_axis_tube'`、`derived_ordinal = 1` |
| 并成一件的容器管身（E3D 把隐式管写在容器那一组里时） | 整组 `<属主>:TUBES`，组内每根 `<属主>:TUBI:<n>`，E3D 多画的记录 `<属主>:TUBES:R<n>` | `derived_kind = 'container_tubes'` / `'container_record'`；组内逐根的判档带 `container_` 前缀 |
| refno 没解出来的 | `<单元 w0>_<单元 w1>~<判档>~<显示名>`（确定的：同一件每次得同一个键） | `refno_resolved = false`，没有 `refno` 这一格 |

refno 怎么来的：RVM 里没有 refno，比对按名称配对。生成侧每发出一个成员就记下它出自哪个元素
（`e3d_model::rvm_export::MemberSource`），比对完按成员路径接回来；只在基准里有、我们没生成的构件按 E3D 的
显示名逆着解（命名元素查库的 NAME 表；`BOX 2 of EQUIPMENT /E` 先解属主、再在成员表里数同名词的第 2 个；
`TUBE n of BRANCH /B` 记成 BRAN 名下的派生件）。解不出来的（基准导出之后那个构件在库里被删了、改名了）
照样留一行，标 `refno_resolved = false`。

每行的格：

| 格 | 含义 |
|---|---|
| `refno` `refno_resolved` `derived` `derived_kind` `derived_ordinal` `tube_from` `tube_to` | 身份，见上表 |
| `dbnum` `noun` `name` `pair_key` | 库号；名词（库里的短名，管身是 `TUBI`）；RVM 里的显示名；配对用的主键（配上的件才有，差异表里这一件的 `name` 写的就是它） |
| `unit_refno` `unit_kind` `root_refno` | 所在的核对单元（按 ZONE 跑是那个 ZONE / 兜底单元，整根跑是 SITE / APPLDW）与上级根。页面单根按同一条规则归到它所在的单元名下 |
| `scope_kind` `scope_refno` | 这一行是只核对单元里一块时写的（`--scope`，或页面上比的是单元之下的节点）：`node` / `subtree` 与那个节点 |
| `run_id` `batch_id` `source` | 写下这一行的那次运行（回到 `rvm_check_run`）、批次、`page` / `batch` |
| `grade` | 判档：`exact` / `chord` / `reference_artifact` / `film` / `off`（配上的件）、`only_baseline` / `only_generated`、`unkeyable_baseline` / `unkeyable_generated`、`centreline_only`（E3D 只画中心线，不进判据）、`container_exact` / `container_chord` / `container_off` / `container_only_baseline` / `container_only_generated`（容器组内逐根账） |
| `passed` `kind_match` `severity` | 这一档算不算通过；两侧图元种类是否一致（没配上的件没有）；`ok` / `info` / `warning`（只是种类不一致）/ `error` |
| `max_axis_delta_mm` `significant_max_axis_delta_mm` | 最大轴向偏差（原始的整盒差）；`reference_artifact` / `film` 两档另有滤掉断开薄片 / 贴体薄膜之后的差，它们凭这个数算通过 |
| `baseline_kinds` `generated_kinds` `baseline_aabb` `generated_aabb` `aabb_frame` | 两侧图元种类；两侧包围盒 `[xmin, ymin, zmin, xmax, ymax, zmax]`（mm）与它们所在的帧：`local`（元素局部系，常态）/ `world`。只在一侧的件（`only_*` / `unkeyable_*`）没有包围盒和轴差：比对报告里它们只有名字 |
| `reason_kind` `reason` | 有问题时的原因种类与带数值的那句话，与差异表里这一件的那一行逐字相同；没有问题的件（`severity = 'ok'`）没有 |
| `finished_at` `finished_unix_ms` | 完成时间 |

规矩：

- **只留最后完成的那一次。** 同一个构件再次被核对时覆盖旧行（`run_id` / `batch_id` 跟着换）。按
  `finished_unix_ms` 比先后：`--resume` 补入库的老结果不会盖掉更新的行；那个单元已经有更晚的一次核对时，
  老结果的构件行整个不写（控制台会说），运行行与差异行照补。
- **重新核对后清陈旧行。** 一个单元核对完，记在它名下、比这次更早、这次没再出现的行清掉（构件删了、挪走了）。
  整根核对清这个根下的全部旧行（包括此前按 ZONE 写的）；只核对一块时只清同一范围上次留下的，范围之外的行不动。
- **没比对出结果就不动这张表。** 生成失败、缺基准、超时这些运行只写运行行与一条 `stage_failure`，上一次的
  构件行留着。
- **按构件看差异就查这张表。** 差异表没有加 refno。这张表里 `severity` 是 `error` / `warning` 的行与比对出来的
  差异行一一对应（`warning` 是包围盒通过、只是图元种类不一致的件，它的 `passed` 仍是 true），原因种类、原因原文、
  轴差、两侧图元和包围盒都相同，另带 refno。
- 一个单元各档的行数与它运行行里的 `counts` 逐格对得上：`exact` … `off` 各对各的格（合起来是 `both`），
  `only_*` / `unkeyable_*` 同名，`centreline_only` 对 `centreline_only_baseline`，`container_off` /
  `container_only_*` 同名（`container_exact` / `container_chord` 在 `counts` 里没有格）。之后别的运行重新核对了
  其中的构件，那几行就归到新的那次名下。
- 作业目录里多一份 `element-checks.json`（紧凑 JSON，每行约 0.5 KB，整根 8 万件约 40 MB）：生成即比对时子进程
  经它把构件级结果交给父进程，`--resume` 补入库也读它。加这份文件之前跑的产物目录补不了构件行，要重跑。
  不走生成即比对时，生成侧另落 `ours/member-sources.json`（每个成员出自哪个元素），比对那一步按它接回 refno。

### 查询

六种查询，命令行与服务端给的是同一份 JSON（`store::answer_on`）：

| 查什么 | 命令行 | 服务端（GET） | 应答 |
|---|---|---|---|
| 按上级根汇总 | `rvm-check-query rollup` | `/api/v1/rvm-check/rollup` | `{count, scanned, truncated, roots: [...]}`：每批、每个库、每个上级根、每种跑法（`mode` = `root` 整根 / `zone` 按 ZONE 拆开）一行，带 `runs`、`units`、`status`（各终态个数）、`clean`、`not_clean`（没过的单元，最多 50 个）、`counts`（各单元之和）、`mismatch_total`、`seconds`、`gen_peak_private_mb`（最大值）。只汇总批量行；一次最多读最新的 20,000 行 |
| 运行记录 | `rvm-check-query runs` | `/api/v1/rvm-check/history` | `{count, runs: [...]}`，一个根 / 单元一行，最新在前 |
| 差异明细 | `rvm-check-query mismatches` | `/api/v1/rvm-check/mismatches` | `{count, rows: [...]}` |
| 差异按原因计数 | `rvm-check-query reasons` | `/api/v1/rvm-check/reasons` | `{count, kinds: [{reason_kind, category, severity, n}]}`，多的在前 |
| 构件级结果 | `rvm-check-query elements` | `/api/v1/rvm-check/elements` | `{count, more, rows: [...]}`：一个构件一行，每行多一格 `key`（记录 id 的键）；`more` 是这一页之后还有没有。按记录 id 的顺序（不按时间排），同一组条件翻页顺序不变 |
| 构件按判档计数 | `rvm-check-query element-grades` | `/api/v1/rvm-check/element-grades` | `{count, rows, passed, not_passed, grades, counts, units: [...]}`：顶层是命中行的合计；`units` 按「哪次运行写的、记在哪个单元名下」分组，每组带 `run_id` `unit_refno` `root_refno` 与同样五格。`counts` 是把各档行数摆回 `rvm_check_run.counts` 的形状，拿来与运行行对账 |

命令行查询直接连 `DbOption.toml` 指的那台库，**只读**（只连接、登录、`SELECT`；不重载函数定义、不建表），
服务不必开着，批量命令正在跑的时候也能查。标准输出只有那一份 JSON，可以直接接管道；`--out FILE` 写进文件。

```powershell
# 这一批按上级根汇总：每个 SITE / APPLDW 下多少单元、过了多少、计数合计
aios-database rvm-check-query rollup --batch-id accept-20261110/7999-zone
# 一个 SITE 的历史：它自己的整根行，加上它下面的全部 ZONE 单元（跨批）
aios-database rvm-check-query runs --root 24383/80211 --limit 200
# 只看某一批里没过的单元
aios-database rvm-check-query runs --batch-id <批> --verdict fail
# 一个 SITE 下全部差异 / 按原因计数
aios-database rvm-check-query mismatches --root 24383/66456 --severity error --limit 100
aios-database rvm-check-query reasons --batch-id <批>
# 一个构件最近一次核对的结果：它自己那一行，加上记在它名下的派生件（给 BRAN 的 refno 就带出它的隐式管身）
aios-database rvm-check-query elements --refno 24383/66459
# 一个 ZONE 下没通过的构件；一个 SITE 下超差的 BOX；翻页
aios-database rvm-check-query elements --unit-refno 17496/8516 --passed false
aios-database rvm-check-query elements --root 24384/22399 --grade off --noun BOX
aios-database rvm-check-query elements --batch-id <批> --limit 500 --offset 500
# 这一批每个单元各档有多少构件（counts 与 runs 里同一个单元的 counts 逐格相同）
aios-database rvm-check-query element-grades --batch-id <批>
# 接管道
aios-database rvm-check-query rollup --dbnum 7999 2>$null | ConvertFrom-Json
```

过滤条件（不给就不限）：`rollup` / `runs` 认 `--batch-id --root --refno --dbnum --unit --status --verdict --limit`；
`mismatches` / `reasons` 认 `--batch-id --root --refno --dbnum --run-id --noun --category --reason-kind --severity --limit --offset`；
`elements` / `element-grades` 认 `--refno`（构件自己的 refno）`--unit-refno --root --dbnum --batch-id --run-id --noun --grade
--severity --passed --kind-match --derived --refno-resolved --limit --offset`。
服务端的参数名与库里的字段名相同（`root_refno`、`reason_kind` …）：

```text
/api/v1/rvm-check/rollup?batch_id=…&dbnum=7999&root_refno=24383/80211
/api/v1/rvm-check/history?refno=24383/73927&root_refno=…&unit=zone&dbnum=7999&verdict=fail&batch_id=…&before=<finished_unix_ms>&limit=50
/api/v1/rvm-check/mismatches?run_id=…&refno=…&root_refno=…&dbnum=…&category=off&reason_kind=aabb_shifted&noun=CTRAY&severity=error&limit=500&offset=0
/api/v1/rvm-check/reasons?dbnum=7022            → 按 reason_kind / category / severity 的计数，多的在前
/api/v1/rvm-check/elements?refno=24383/66459    → 这个构件（连同记在它名下的派生件）最近一次核对的结果
/api/v1/rvm-check/elements?unit_refno=…&root_refno=…&dbnum=…&batch_id=…&run_id=…&source=batch&noun=ELBO&grade=off&severity=error&passed=false&kind_match=false&derived=true&refno_resolved=false&limit=500&offset=0
/api/v1/rvm-check/element-grades?batch_id=…     → 条件同 elements；按运行与单元分组的各档计数
```

直接查库的写法一样：

```sql
SELECT reason_kind, noun, count() AS n FROM rvm_check_mismatch WHERE dbnum = 7022 AND severity = 'error' GROUP BY reason_kind, noun;
SELECT run_id, verdict, mismatch_by_kind FROM rvm_check_run WHERE refno = '23406/1' ORDER BY finished_unix_ms DESC;
-- 一个 SITE 下的 ZONE 单元（2026-11-10 之后的行）
SELECT refno, name, status, counts FROM rvm_check_run WHERE root_refno = '24383/80211' AND unit = 'zone' AND batch_id = 'accept-20261110/7999-zone';
-- 一个构件的核对结果：按记录 id 直接取，或按 refno 取（连同记在它名下的隐式管身）
SELECT * FROM rvm_check_element:⟨24383_66460⟩;
SELECT * FROM rvm_check_element WHERE refno = '24383/66459';
-- 一个 ZONE 下各档各有多少构件；全库 refno 没解出来的
SELECT grade, count() AS n FROM rvm_check_element WHERE unit_refno = '17496/8516' GROUP BY grade;
SELECT name, grade, unit_refno FROM rvm_check_element WHERE refno_resolved = false;
```

量级（草稿库上 8 万行、8 个索引）：按记录 id 取 0.1 ms，按 refno 0.5 ms，一个单元的第一页 500 行 9 ms，
翻到第 7 万行之后的一页约 0.3 s，一个 8 万行的单元按判档计数约 1.2 s。

## 2026-11-07 实测（AMS 九库 19 根，本机 32 核 / 64 GB）

全部 `pass`（`/1AXIS` 为 `empty`）：both 243,522 / exact 243,522，off、only_*、unkeyable、
kind_differ 全 0。E3D 导出合计 441 s（每库一次会话，ams7333 两根 211 s），生成 175 s（ams7333
两根各 9.1 GB 峰值），比对 8.9 s；整批 10 分钟出头，产物 5.0 GB。
