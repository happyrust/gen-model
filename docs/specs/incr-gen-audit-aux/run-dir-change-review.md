# `run_dir` 改动审核：回放被测服务的 CWD 从 `repo()` 改为运行目录 + staging `resource/surreal`（2026-10-27）

> 审的是 `tests/increment_replay/main.rs`（2732 行）里这一组改动：`spawn_server` 用 `.current_dir(run_dir)`（:688）、`stage_surreal_scripts`（:636-668）在 `Service::start` 里先于 spawn 调用（:740-741）、源码钉 `the_service_under_test_runs_from_the_run_directory_with_staged_surql_scripts`（:2697-2732）。
> 证据都是工作区文件 + 行号（2026-10-27）；不跑命令，vendor `aios_core` 的连库行为只能引本仓注释与 changelog。

## 0. 判定

**正确，且两个声明的问题都解决了**：「抢仓根单实例锁」和「往仓根写 dblog 与 accel_tree」。顺带解决了计划里记的「回放任务写进仓根 `logs/`」（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md:12`）。残余风险 10 条（§2），缺的断言 7 条（§3），没有一条是阻塞性的。

## 1. 逐条证据

### 1.1 单实例锁不再与常驻开发服务相撕

| 事实 | 出处 |
|---|---|
| 锁文件名 `.gen-model.instance.lock`，路径 = 进程 CWD（`std::env::current_dir()`） | `src/lib.rs:167`、`:180-184` |
| 只有 `StoreMode::Mem` 不拿锁；`rocksdb` 照常拿 | `src/lib.rs:226-230`、`:237-242` |
| harness 强制 `AIOS_STORE_MODE=rocksdb` | `tests/increment_replay/main.rs:690` |
| 被测服务 CWD = `run_dir`；`run_dir = <repo>/output/increment-replay/<case>-<stamp>`，每条用例一个 | `main.rs:688`、`:1407-1412` |
| 冷重启复用同一个 `self.run_dir`，先 `kill` + `wait` 再 spawn（旧句柄已释放） | `main.rs:786-793` |
| 机器级写者锁按 `v_ip:v_port` + ns + db 计，不按 CWD | `src/data_interface/database_writer_lock.rs:40-47` |
| 每条用例自起一台临时 surreal，派生配置改 `v_port` 指向它 | `main.rs:716-729`、`:542`、`:1436-1449` |

结论：锁按 CWD 隔离，运行目录每用例一个 → 与仓根常驻服务（8022）无交集；写者锁因端口不同也无交集。以前的失败形状「`stage=instance-lock` 退出 1」（`main.rs:2691-2693` 注释）不再可能由仓根锁引发。

### 1.2 dblog / accel_tree / logs / cata_dep_cache 落进运行目录

| 产物 | 路径怎么算 | 出处 |
|---|---|---|
| `<stamp>_dblog.txt` | 纯文件名，相对 CWD；只在 `enable_log=true` 时写 | `src/lib.rs:662-674` |
| `accel_tree_<project>.snapshot` / `.bin` / `.meta.json` | 纯文件名，相对 CWD；`write_file_atomic(path)` 直接用该名 | `src/fast_model/aabb_tree.rs:132-134`、`:137-142`、`:182-187`、`:150-165` |
| `logs/batch-failures-*.jsonl`、`logs/task-changes-*.jsonl`、`logs/task-history-*.jsonl`、`logs/queue-stalls-*.jsonl`、`logs/process-events.jsonl` | `const DIRECTORY = "logs"` / `Path::new("logs")`，相对 CWD | `src/data_interface/batch_failure_log.rs:35-36`、`task_change_list.rs:37`、`task_history_log.rs:28`、`queue_stall_diagnostics.rs:44`、`:170`、`src/process_diagnostics.rs:66`、`:103` |
| `cata_dep_cache.bin` | 运行目录相对（lib.rs 自述） | `src/lib.rs:222-225` |
| 网格目录（唯一被点名的「绝对路径、共享」产物） | `PLANT_ASSET_ROOT=<run_dir>/assets` 压掉配置 `meshes_path` | `main.rs:685`、`:693`；`src/options.rs:825-840`；提示句 `src/lib.rs:204-209` |
| 被测配置 `DbOption.toml` | 派生到 `run_dir`，`DB_OPTION_FILE` 指向它 | `main.rs:684`、`:689`、`:1437-1449` |

结论：所有 CWD 相对的运行期文件随 CWD 一起搬进 `run_dir`；唯一绝对路径的共享面（网格目录）被环境变量压成 `run_dir/assets`。仓根 `.gitignore` 对这些名字的忽略（`accel_tree*` :12-15、`/.gen-model.instance.lock` :97、`/logs/` :102、`/*_dblog.txt` :151）从此只是对常驻服务的兑底，不再替回放兑底。

### 1.3 staging `resource/surreal` 是必要的，顺序也对

| 事实 | 出处 |
|---|---|
| `aios_core` 连库时按 CWD 相对路径 `resource/surreal` 装函数脚本，目录不在整个 connect 失败（`Failed to define common functions … (os error 3)`） | `main.rs:630-635`、`src/bin/l3_suite/fixture.rs:752-759`；历史现场 `changelog.md:1741-1742`（l3 夹具栈第一次从 `run_dir` 起时撞过同一错） |
| 本仓 `run_app` 里的「目录缺失走内置快照兑底」排在连库之后，救不了 connect | `src/lib.rs:960-976` |
| staging 把仓根 `resource/surreal` 顶层普通文件拷到 `run_dir/resource/surreal`，源目录缺或一个没拷到就 panic，幂等覆盖 | `main.rs:636-668` |
| 当前仓根脚本 4 个，全在顶层：`common.surql`、`fn_query_room_code.surql`、`fn_query_room_code_hh.surql`、`material_common.surql` | `resource/surreal/` |
| `Service::start`：surreal 起来 → `stage_surreal_scripts(&repo(), run_dir)` → `spawn_server` | `main.rs:739-741` |
| 钉：`spawn_server` 体含 `.current_dir(run_dir)`、不含 `.current_dir(repo())`；`start` 体里 `stage_surreal_scripts(` 的位置在 `spawn_server(` 之前 | `main.rs:2698-2731` |
| `l3_suite` 夹具栈同款实现与同款钉（还多一段真目录拷贝断言） | `src/bin/l3_suite/fixture.rs:760-791`、`:2939-2969` |

## 2. 残余风险（10 条）

| # | 风险 | 出处 | 程度 |
|---|---|---|---|
| R1 | 钉是**源码文本钉**（`include_str!("main.rs")` 找子串）：改写成 `.current_dir(&run_dir)` / `.current_dir(run_dir.as_path())` 会假红；换个变量名指回仓根（如 `.current_dir(repo_root)`）会假绿 | `main.rs:2698-2714` | 中 |
| R2 | 没有任何**运行期**断言证明锁与产物真的落在 `run_dir`：用例跑完不查 `run_dir/.gen-model.instance.lock`、不查仓根有没有新增 `*_dblog.txt` / `accel_tree*` / `logs/`；服务在 `/health` 前退出只报 `服务进程已退出 <status>` + 日志路径，不读日志里的 `stage=` 点名原因 | `main.rs:989-996`、`:1452-1460`、`:796-804` | 中 |
| R3 | staging 只拷顶层普通文件，子目录跳过；今天 4 个脚本都在顶层所以无影响，一旦仓根出现嵌套脚本，运行目录的函数集就与仓根不同（`fixture.rs` 的钉还把「跳过 `nested`」当成约定钉住了） | `main.rs:650-653`；`fixture.rs:774`、`:2962` | 低 |
| R4 | `restart` 不重新 stage，钉只盯 `start` 的顺序；只要 `run_dir/resource` 在用例中途被清（今天没有这样的步），冷重启会在 connect 失败并以 180 s 超时收场 | `main.rs:786-792`、`:2715-2731` | 低 |
| R5 | `run_dir` 在仓内 `output/increment-replay/`：`/output/*` 已忽略，但每次运行一台 rocksdb `store` + 日志永不清理，`git clean -fdx` / IDE 索引都会碰到；「不污染检出」对受管文件成立，对磁盘不成立 | `main.rs:1407`、`:717`；`.gitignore:126` | 低 |
| R6 | 仓根已有的残留（大量 `*_dblog.txt`、`accel_tree_AvevaMarineSample.snapshot`、`accel_tree.bin`）不随这次改动清除；常驻服务还在往仓根写，所以「仓根文件集不变」这种全局断言写不出来，只能按用例起止时刻 / 文件名过滤 | 仓根目录；`main.rs:2692` | 低 |
| R7 | `l3_suite` 非夹具栈（AMS 金基线模式）仍 `.current_dir(&paths.repo)`，同一类抢锁 / 污染问题在那条路上还在（不在本次改动范围） | `src/bin/l3_suite.rs:923`、`:948`、`:2244` | 中 |
| R8 | staging 的必要性挂在 vendor `aios_core` 连库行为上（不在本仓可核）；若 vendor 以后不在 connect 时装脚本，staging 变成多余但无害，钉仍会强制它 | `main.rs:632-634`；`src/lib.rs:960-976` | 低 |
| R9 | 派生配置只改六个键，其余照本机 `DbOption.toml`：除 `meshes_path`（被 `PLANT_ASSET_ROOT` 压掉）外，本机配置里其它绝对路径类键是否存在本次未核；`enable_log` 也照本机，为 false 时运行目录里根本没有 dblog 可查 | `main.rs:510-570`；`src/lib.rs:663-665` | 低 |
| R10 | 文档口径落后：10-25 计划 :12 的欠账句未回写，:81 / :89 / :222 仍写 `store_mode=mem`，而 harness 是 rocksdb（`docs/plans/` 不在本次可改范围） | `docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md:12`、`:81`、`:222`；`main.rs:690` | 低 |

## 3. 缺的断言（7 条，都能在 `tests/increment_replay/main.rs` 里加，不动产品代码）

| # | 断言 | 怎么判 | 挡的是哪条风险 |
|---|---|---|---|
| M1 | 服务 `/health` 起来后 `run_dir/.gen-model.instance.lock` 存在，内容 `pid=` 等于子进程 `child.id()` | 锁文件内容格式 `project=…\npid=…\nstarted_at=…`（`src/lib.rs:255-259`） | R2 |
| M2 | 用例前后对仓根顶层文件快照，差集里没有新增 `*_dblog.txt` / `accel_tree*` / `.gen-model.instance.lock`（按 mtime ≥ 用例开始时刻过滤，排除常驻服务正在写的那一个 dblog） | `fs::read_dir(repo())` 两次比对 | R2、R6 |
| M3 | 任一任务落终态后 `run_dir/logs/task-history-*.jsonl` 存在且非空 | `src/data_interface/task_history_log.rs:8`、`:28` | R2；直接验证 10-25 计划 :12 那条欠账 |
| M4 | `run_dir/resource/surreal/*.surql` 与仓根同名文件逐字节相同；服务 stdout 含 `surql-functions … source=disk`，不含 `source=builtin-fallback` | `src/lib.rs:969`、`:975` | R3、R8 |
| M5 | 源码钉补一句：`restart` 体里 `spawn_server(` 的第二个实参是 `&self.run_dir`（冷重启不换目录） | `main.rs:791-792` | R4 |
| M6 | 服务在 `/health` 前退出时，`assert_alive` 读 `stdout.log` 找 `stage=` 那一行并把它写进 panic 文案（`stage=instance-lock` 就是改动要消灭的那个形状） | `main.rs:989-996` | R2 |
| M7 | 给 `stage_surreal_scripts` 补一段临时目录上的真拷贝测试（拷到了、子目录跳过、源目录缺 panic），与 `fixture.rs:2959-2969` 对齐；现在 `main.rs` 的钉只钉调用顺序 | `main.rs:2722-2731` | R1、R3 |
