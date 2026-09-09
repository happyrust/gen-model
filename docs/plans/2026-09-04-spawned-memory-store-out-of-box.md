# gen-model 自带 memory 后端 surreal 子进程（A′）· 开箱即用形态开发计划

- 日期：2026-09-04
- 拍板：用户「我要 A′：让 gen-model 自己拉起 memory 后端的 surreal 子进程，plant-ui 零改动」
  （承接 `plant-ui/docs/plans/2026-09-04-kv-mem-out-of-box-adaptation.md` 的 D1）
- 范围：`src/lib.rs`（连库那一段）、`src/options.rs`（形态开关）、新增 `src/store_process.rs`、
  `src/web_service/handlers.rs`（`/health` 两格）、`src/fast_model/model_db_adapter.rs`（介质口径）、
  `scripts/`（版本闸规则的单一来源）。
- 状态：**Q5 已翻案（2026-09-07 用户拍板：默认翻回 `external`）**。三档形态、署名裁决、
  `/health` 口径、子进程生命周期**全部照旧有效**，翻掉的只有「哪一档是默认」这一格——
  `spawned-mem` 从此要显式写。翻案的经过与理由见 §9。
  **Q1–Q4 已定案**（2026-09-04 用户拍板：全按 §4 推荐项）。
  **P0 / P1 / P2 / P3 / P4 已于 2026-09-04 全部落地**（见 §5 表内与下方记录）。
  余下只剩 P5（版本闸单一来源护栏）与 **P6（实测验收，一次都没跑过——下面全部结论都是单测与
  `#[ignore]` 真库用例给的，没有任何一次真的起过服务）**。
- **读下文的注意**：§3.1 / §3.8 / §4 Q5 / §5 P3 里凡是说「默认 `spawned-mem`」的地方，
  记的都是 2026-09-04 那一版的口径，**今天不成立**。留着不改是因为 P0 那份迁移清单
  （13 份配置显式写 `external`）本身还在生效，而且它正是 §9 翻案时唯一没白做的那一半。

### P0 落地记录（2026-09-04）

| 改动 | 位置 | 说明 |
|---|---|---|
| 新增 `StoreMode` 三值 + `store_mode()` + `AIOS_STORE_MODE` + `store_mode_notice()` | `src/options.rs`（`in_memory_db` 之后） | `is_durable()` 只对 `External` 回真；`has_endpoint()` 只对 `EmbeddedMem` 回假——这两句是 `/health` 与回执介质口径的唯一来源 |
| `store_mode` 进 `DbOptionExt` / `DbOptionExtFields` / `From<DbOption>` / 覆盖合并 | 同上 | 既有护栏 `the_notice_names_every_field_the_table_actually_carries` 自动跟上，无需手改 |
| **默认值不翻**，护栏钉住 | `introducing_store_mode_does_not_flip_the_default_yet` | 翻转与接线（P3）一起做 |
| 13 份夹具 / 现场配置补 `store_mode = "external"` | `db_options/*.toml` 8 份 + 仓根 `DbOption-{ams,aba,KMX,zsy}.toml`、`DbOption_text.toml` | 仓根 `DbOption.toml` **不写**：它就是开箱即用形态本身 |
| 护栏 `every_db_option_fixture_declares_its_store_mode` | `src/options.rs` tests | 扫上述两个目录；**已按负例验过**：注释掉一行即红，错误里点名是哪一份 |

**验证**：`cargo test --lib -- options::tests` 55 passed / 0 failed（新增 6 条）；
`cargo check --lib --tests` 通过；`rustfmt --check --edition 2024` 通过。**未 live**（P0 不改行为，没有可 live 的东西）。

### P1 落地记录（2026-09-04，Q1–Q4 全按推荐项拍板之后）

新增 `src/store_process.rs`（`lib.rs` 已挂 `pub mod store_process;`），**仍然没有调用方**——接线是 P3。

| 事 | 落地形态 | 与 §3.2 的差别 |
|---|---|---|
| 二进制查找 | `surreal_exe_candidates(env, exe_dir, repo_root)` 纯函数 + `resolve_surreal_exe()`；名单去重；空环境变量当没设 | 无 |
| 版本闸 | `version_gate()` 返回 `Result<Option<警告>, 拒绝>`：`major != 2` 硬拒并点名两个坑；rev 不符只警告；拿不到 `Cargo.lock` 不编造警告 | 无 |
| 地址 | **新增 `resolve_bind()`**：`localhost` → `127.0.0.1`（`--bind` 只吃 socket 地址）；`0.0.0.0` 探回环；**其余主机名早拒并给出路** | §3.2 没提这一格。不做的话 surreal 会在参数解析阶段就退出，日志里只剩一行用法提示 |
| 数据源参数 | `memory` 是**位置参数**，护栏 `the_datastore_is_a_positional_argument_not_a_flag` 钉住 | §3.2 写的 `--memory` 是错的，2.1.x 上是「未知参数」 |
| 日志 | `logs/surreal-<port>.log`，不继承控制台；超时错误带 20 行尾巴，「一个字都没输出」单独成话；起不来的那个也收掉 | 无 |
| 收尸 | `shutdown()` 幂等，`Drop` 与 `shutdown_spawned_store()` 各调一次；Windows 走 `taskkill /T /F` | `ctrl_c` 处理器**没在这一步装**——它属于 `run_app` 的关停路径，跟着 P3 一起接 |
| 进程内单例 | `install_spawned_store()` / `shutdown_spawned_store()` / `spawned_endpoint()`；重复安装会先收掉旧的 | §3.2 没提，P3 与 P4（`/health`）都要用 |

**验证**：`cargo test --lib -- store_process::` **16 passed / 0 failed / 1 ignored**；
`#[ignore]` 那条**真起真杀**手动跑过并通过（`bin/surreal.exe` 未纳入 git，CI 上没有）：
`pid=3468 监听 127.0.0.1:18009`、二进制自报 `2.1.4+20250317.45013fc9`（rev 与锁一致、无告警）、
`shutdown()` 之后端口确实放开、事后 `Win32_Process` 里无本轮残留。
`cargo check --lib --tests` 与 `rustfmt --check --edition 2024` 通过。

**P1 没做的**：`ctrl_c` 挂钩（随 P3）。

### P2 落地记录（2026-09-04）

同一个文件里加了署名与四格裁决，**仍然没有调用方**。

| 事 | 落地形态 | 与 §3.3 的差别 |
|---|---|---|
| 署名住哪 | 库里 `gen_model/store` 的 `owner:current` 单行；`owner_pid` / `child_pid` / `nonce` / `project` / `namespace` / `heartbeat_ms` / `started_at_ms` | §3.3 写的表名 `__gen_model_store` 换成不需要转义的 `owner`，并**固定 ns/db**——配错工程的那个 gen-model 也要读得到它 |
| 活没活 | **心跳**（10 s 敲一次，60 s 判停），pid 只用于「判停之后确认它确实不在」与「收孤儿时杀谁」 | §3.3 原本打算靠 `pid + 启动时刻` 这对身份。换掉的理由：跨平台拿另一个进程的启动时刻要么依赖 WMI、要么加 crate，而心跳把同一件事变成一次读 |
| 四格 | `Idle` / `Attach` / `ReclaimOrphan` / `Refuse`，**外加**两格并进 `Refuse`：心跳停但 pid 还在、心跳停但查不出 pid | §3.3 只列了三格。这两格是实现时冒出来的——它们是「分不清」，而这张表唯一的破坏性动作是杀进程，所以一律不猜 |
| 读不出署名 | `SignatureProbe` 三态（`Found` / `Absent` / `Unreadable`）分开报，处置相同（都拒） | §3.3 把「读不到」笼统当成别人的库。分开是为了让错误里带上原话（`authentication` 之类），否则人不知道该去看凭据还是看端口 |
| 收孤儿 | 杀完**等端口真的放开**再起，超时则响亮失败 | §3.3 没提。不等的话紧接着那次 `--bind` 会撞 `address already in use`，而那条错误只落在子进程日志里 |
| 注入 | `project` / `namespace` 来自配置，进语句前转义单引号；护栏钉住 | §3.3 没提 |

**验证**：`cargo test --lib -- store_process::` **29 passed / 0 failed / 2 ignored**；两条 `#[ignore]` 均手动跑过通过。
第二条 `a_real_store_answers_with_the_signature_we_stamped` 是**署名走真库**：验了盖之前 `Absent`、
盖之后原样读回、判据落 `Attach`、重复盖不留第二行。**这条不能只靠纯函数**——语句形状或字段名对不上时的
表现是「读不出署名 → 拒绝启动」，看上去像端口被别人占了。
`cargo check --lib`（含 test profile）与 `rustfmt --check --edition 2024` 通过。

`process_exists` 在非 Windows 上走 `kill -0`，**没有在 Linux 上验过**。

### P3 落地记录（2026-09-04）—— **这一步起行为真的变了**

| 事 | 落地形态 |
|---|---|
| 连库分派 | `run_app` 的 ws 段从二分支换成 `match store_mode()` 三分支。`spawned-mem` 先 `ensure_own_store()` 再 `connect_configured_store()`；`external` 直接后者。**两档连的是同一句 `init_surreal()`** |
| 默认值 | 两个键都没写 = `spawned-mem`；**写了 `in_memory_db = false` 而没写 `store_mode` 的回 `external`**——那句 `false` 是人写下的意图，不该被新默认盖掉（升级时最容易被静默改掉的一格） |
| 实例锁（Q4） | `instance_lock_skip_reason` 改吃 `StoreMode`，只有 `embedded-mem` 跳过；护栏改名 `only_the_embedded_store_needs_no_single_instance_lock`，三档各钉一句 |
| `Ctrl+C` | 起完自带存储就挂钩子。`Drop` 兜正常退出与 panic，兜不住 `Ctrl+C`——那一路 tokio 直接结束进程、析构不跑。**少这一条是每一次 Ctrl+C 都漏一个占着端口的孤儿** |
| 横幅 | 三档分别说话；`embedded-mem` 那句的出路从「用部署脚本的 -InMemory」改成「改 `store_mode = "spawned-mem"`」 |
| 护栏锚点 | `startup_cleanup_precedes_spatial_watcher_worker_and_model_startup` 原先钉字面量 `match init_surreal().await`，连库已不只一条路 → 换成 `match crate::options::store_mode()`。这条测试守的是**顺序**，不是某条连接语句的样子 |

**验证**：全量 `cargo test --lib` **1508 passed / 4 failed / 103 ignored**。四条失败**全部与本改动无关且都验过**：

- 三条 `increment_manager::tests::*scope*` —— 工作树里 `DbOption.toml` 今天启用了
  `watch_dbnums = [7998, 8000]`（另一会话的 S5 live 验收窗口），而那三条断言 7997 / 1001 在范围内。
  用一份没有该键的配置复跑（`DB_OPTION_FILE=db_options/DbOption-e2e-8009`）：同一批 **44 passed / 0 failed**。
- 一条 `the_embedded_panel_is_real_and_a_disk_copy_still_wins` —— `web/ops.html` 改动中的已知红
  （09-04 审核 §6 已记）。

新增 `#[ignore]` 的**整条路径**用例 `ensure_spawns_once_and_attaches_the_second_time` 手动跑过并通过：
第一次起（pid=37320 @ 18011）→ 第二次附着 → 换一份工程身份被拒。`rustfmt --check --edition 2024` 通过。

**P3 没做的**：探针与 python `full_init` 那几个自己 `init_surreal()` 的入口还没收进统一的
`ensure_connected()`（Q1 的后半）。服务在跑时它们照旧连得上（附着是自然发生的），服务没起时与今天一样
连不上——不是回归，但 Q1 的账没平。

### P0 补齐两半 + P4 的 `/health` 半边落地记录（2026-09-04）

| 事 | 落地形态 |
|---|---|
| 「先起库再起服务」五处补 `AIOS_STORE_MODE=external` | `bin/l3_suite/fixture.rs`、`Run-LiveBatch.ps1`、`Run-Issue7E2E.ps1`、`Run-RoomE3DE2E.ps1`、`Start-AiosDatabaseManual.ps1`。它们指的都是已经写了 `external` 的 `db_options/*` 配置，环境变量是**第二道**；`Run-LiveBatch.ps1` 例外——**一部分 live 用例不读 `DB_OPTION_FILE`**，只能靠环境变量收口 |
| CI | `windows-tests.yml` 只跑 `cargo test`、不起服务，无需改（已核） |
| `Start-Surreal8009.ps1` 改口 | `.SYNOPSIS` 加「**只有 `store_mode = "external"` 的部署才需要它**」；`.DESCRIPTION` 开头说清默认形态下不要跑它，以及「先手工跑了它又忘了改配置 → 服务会明确报错而不是悄悄连上你这台库」——那正是 P2 的署名判据买到的东西 |
| `/health.sul_db` | 加 `medium`（三值）与 `durable`；**`spawned-mem` 的 `endpoint` 报真地址**。不改就是个坑：那台库是本进程起的但端口是真的，报 `embedded:mem` 会让跨部署互踩探测**放行两个抢同一个端口的实例** |
| `/health.in_memory_db` | 留给老客户端，语义收窄成「持久层不落盘」（`!durable`）。它**不回答「连不连得上」** |
| 护栏 | 新增 `only_the_embedded_store_reports_a_placeholder_endpoint`（分叉判据必须是 `has_endpoint()`，且 `health` 全文不再直接读退役的两态开关）；既有 `sul_db` 六键承诺扩成八键 |

**验证**：全量 `cargo test --lib` **1511 passed / 4 failed / 104 ignored**（四条失败同上，均已验与本改动无关）；
`rustfmt --check --edition 2024` 通过。

### P4 收尾记录（2026-09-04）

| 事 | 落地形态 | 与 §3.6 / §3.7 的差别 |
|---|---|---|
| `PersistMedium` 三值 | `RocksDb` / `SpawnedMem` / `EmbeddedMem`，`PersistTarget::process()` 按 `store_mode()` 逐档判 | §3.6 建议的名字是 `External`；改叫 `RocksDb` 是因为它还要盖住不带 `ws` 的 `local` 构建那条进程内嵌 rocksdb——**这一档要紧的是「活得过重启」，不是「在不在外面」** |
| 新增 `is_reachable()` | `spawned-mem` **不 durable，但别的进程读得到**。两件事分开问 | §3.6 没提。不分开的话回执上「不 durable」会被读成「读不到」 |
| `ensure_store_connected()` | 连持久层的**唯一入口**，`run_app` 与旁路入口共用；配套 `attach_store_connected()` | §3.7 只说了「收进 `ensure_connected()`」，形状在这里定死 |
| **`StoreStartup::{Spawn, AttachOnly}`** | 只有服务主进程给 `Spawn`。探针（`direct_model_persist` / `db_window_repair` / `world_refno_probe`）、MCP `query_service` 独立路径、python `full_init` 与 `connect` 一律 `AttachOnly` | **§3.7 缺了这一格**，而它是这一步真正要紧的东西：端口空着时 `AttachOnly` **响亮地说「服务没起」**，不自己拉一台空库在上面作业——后者的失败一声不响（「查了半天什么都没有」「纠正跑完什么都没变」「取证落完随进程消失」）。撞上自家孤儿同样拒绝：探针没有资格杀别人留下的进程 |
| 护栏锚点 | `startup_cleanup_precedes_…` 改钉 `ensure_store_connected(&db_option`——分派已从 `run_app` 搬走 | — |

**验证**：全量 `cargo test --lib` **1511 passed / 4 failed / 104 ignored**（四条同上，均已验与本改动无关）；
三条 `#[ignore]` 真库用例全部手动跑过通过，整条路径那条已扩到验 `AttachOnly`（端口空着 → 报
「没有自带存储在跑」；起来之后 `Spawn` 与 `AttachOnly` 两档都附着）。
`cargo check --lib`、三个 bin 各自 `cargo check --bin`、`cargo check -p aios-py` 全通过；
`rustfmt --check` 通过（`python/src/lib.rs` 第 261 行有一处既有格式偏差，不是本轮改的，没动它）。

**一条自己造的坑，已经填了**：本轮编辑把 `src/lib.rs` / `src/options.rs` /
`src/data_interface/model_update_pending.rs` 的行尾从 LF 变成了 CRLF，两条 `include_str!` 源码钉护栏
（`\n    crate::…reconcile_model_coverage_at_startup().await;\n`）因此红——**内容完全正确，红的只是行尾**。
已全部改回 LF（`git ls-files --eol` 复核 `w/lf`）。教训写在这里：本仓的源码钉护栏按 `\n` 匹配，
任何把文件写成 CRLF 的编辑都会让它们整批变红，而错误信息完全不指向行尾。
（后续几轮 `handlers.rs` / `Run-LiveBatch.ps1` / `Start-Surreal8009.ps1` 同样中招，同样已改回。
**改完一批文件顺手 `git ls-files --eol` 扫一遍**是本仓唯一可靠的自检。）

---

## 0. 一句话

把「内存形态」的实现从**进程内嵌 `mem://`** 换成**gen-model 自己拉起一个 `--memory` 后端的
surreal 子进程，绑到配置里那对 `v_ip:v_port`**。对下游一个字不改（`SUL_DB` 仍是 ws 连接、
仍走 `init_surreal()`），对 plant-ui 一个字不改（它连的还是 `DbOption.toml` 里那对值），
对运维工具一个字不改（`/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1` 全部重新够得着）。

**换来的**：开箱即用从「三条命令」变成「一条」——起 gen-model，库跟着起来，零预解析、零落盘。
**要付的**：子进程的生命周期、端口冲突、孤儿进程这三件事从此是 gen-model 的账。

---

## 1. 为什么这条路比另外两条便宜

| | 进程内嵌 `mem://`（今天的 `in_memory_db`） | 外置 memory（`Start-Surreal8009.ps1 -Memory`） | **A′（本计划）** |
|---|---|---|---|
| plant-ui | **起不来**（`data.rs::ready()` 第一步连 `v_ip:v_port`，那儿没有端口） | 照常 | 照常 |
| `/sql` 探针 / `rvm_verify` / 取证脚本 | 全部够不着（`options.rs:540` 自己写的） | 照常 | 照常 |
| 用户要敲几条命令 | 1 | 3（surreal → gen-model → plant-ui） | **2**（gen-model → plant-ui） |
| gen-model 要新写的东西 | 0（已有） | 0 | 一个子进程管理模块 |

`options.rs:539-543` 那段注释已经把结论写死了：**「要留证据或要让别的进程读库，就别开
`in_memory_db`——那时该用的是部署脚本的 `-InMemory`」**。A′ 做的就是把那句话里的「部署脚本」
搬进进程自己，让用户不必知道它的存在。

---

## 2. 现状锚点（2026-09-04 工作树实位）

| 事实 | 位置 |
|---|---|
| 内存形态今天连的是 `SUL_DB.connect("mem://")`，不 `signin`（嵌入式引擎没有认证面） | `lib.rs:1223-1243` |
| ws 分支二选一：`in_memory_db()` → `connect_in_memory_store`，否则 `init_surreal()`；连不上就地退出 | `lib.rs:1269-1294` |
| 内存形态**不拿实例锁**，理由是「两个实例不共享库」 | `lib.rs:159-180` |
| 启动横幅在内存形态下说「进程一退整库即消失，外部无端口可连」 | `lib.rs:1185-1191` |
| `in_memory_db` 默认 `false`，`AIOS_IN_MEMORY_DB` 压过配置 | `options.rs:527-556` |
| `/health.sul_db.endpoint` 在内存形态下报 `"embedded:mem"`，其余报 `v_ip:v_port`；这一栏被**跨部署互踩探测**用来决定拦不拦 | `handlers.rs:213-228` |
| `/health.in_memory_db` 已有 | `handlers.rs:451-455` |
| `PersistMedium` / `PersistTarget::process()` 按 `options::in_memory_db()` 判介质，`is_durable()` 据此回话 | `model_db_adapter.rs:98-151` |
| **仓库里已经有一份「拉起 surreal 并等端口」的成品**：`Command::new(surreal).args(["start","--user","root","--pass","root","--bind","127.0.0.1:8048"]).arg("rocksdb:…")` + `wait_port(8048, 60s)` | `bin/l3_suite/fixture.rs:563-580` |
| 配套的子进程栈：`Stack` + `impl Drop` 逆序 `kill_tree(child.id())`；`spawn_logged` 把 stdout/stderr 落文件；`wait_port` / `port_open` 用 `TcpStream::connect_timeout` | `bin/l3_suite.rs:491-529`、`:1266-1274`、`:2440-2457` |
| 版本闸的全部规则今天**只存在于 PowerShell 里**：查找顺序（`-SurrealExe` → `AIOS_SURREAL_EXE` → `bin/surreal.exe`，**不回退 PATH**）、`major == 2` 硬拒、`Cargo.lock` 的 rev 比对（不符只警告） | `scripts/Start-Surreal8009.ps1:59-128` |
| 仓里那个二进制自报 `2.1.4+20250317.45013fc9`，与 `Cargo.lock` 锁的 rev 一致 | `bin/surreal.exe` |
| 3.x 服务端的两种失败：存储版本不可逆迁移提示；`memory` 后端也会 `Server sent no subprotocol` | `readme.md` 开头、`Start-Surreal8009.ps1:10-17` |
| 另有三个入口自己 `init_surreal()`，**不经 `run_app`**：python `full_init` / `lib.rs`、`query_service` 的模型库懒连 | `python/src/exec_api.rs:264`、`python/src/lib.rs:91`、`query_service.rs:206` |
| 冷库下启动不生成任何模型：`gen_root` 零行 → `roots_T \ roots_S` 整个进 `lazy`，等按需 `ensure` | ADR-058 D6 |
| 冷库下 DESI **仍要做一次整库基线**（e3d-io 退化窗口） | ADR-058 D3 |
| 内存形态已实跑过一次：7997 GENERATE 29.2s / PERSIST 22.1s / READBACK 47475+4623，平表三件齐活 | `changelog.md` 2026-09-04 |

---

## 3. 设计

### 3.1 形态怎么命名（决定 Q1）

今天 `in_memory_db: bool` 只有两态，A′ 之后其实有三种介质：**rocksdb（外置）/ memory（外置或自带）/
`mem://`（进程内嵌）**。建议换成一个显式三值：

```toml
# 持久层介质。默认 "spawned-mem" = 开箱即用：gen-model 自己拉起一个 --memory 后端的
# surreal，绑到下面的 v_ip:v_port；进程退出即整库消失，但**端口是真的**，
# plant-ui / /sql 探针 / rvm_verify 照常连得上。
#   "external"    连 v_ip:v_port 那台已经在跑的 SurrealDB（生产落盘形态）
#   "spawned-mem" 自己拉起 memory 后端的子进程（默认）
#   "embedded-mem" 进程内嵌 mem://，没有端口，只有本进程读得到（探针 / 单测用）
store_mode = "spawned-mem"
```

`in_memory_db = true` **保留为 `"spawned-mem"` 的别名**并出一行迁移提示——它散在
`AIOS_IN_MEMORY_DB`、`/health`、`PersistTarget`、烟测脚本与至少两份 ADR 里，一次性删掉的面太大。
`AIOS_STORE_MODE` 压过配置，取值规则同既有那套。

### 3.2 子进程的生命周期

新增 `src/store_process.rs`，只做四件事，**全部照抄 `l3_suite` 那套已经跑了几个月的形状**：

```rust
/// 自带的 memory 后端 SurrealDB 子进程。活着的时候占着 `v_ip:v_port`，
/// `Drop` 时连同它的子孙一起杀（Windows 走 taskkill /T，与 l3_suite 的 kill_tree 同一份）。
pub struct SpawnedStore {
    child: Child,
    endpoint: String,
    exe: PathBuf,
    version: String,
}

/// 起一个，等到端口能连上为止。失败一律响亮返回，不静默回落到别的介质
/// ——回落等于「用户以为自己在内存形态，实际连上了昨天那台落盘库」。
pub fn spawn(db_option: &DbOption, timeout: Duration) -> anyhow::Result<SpawnedStore>;
```

1. **解析二进制**：`AIOS_SURREAL_EXE` → 可执行文件同目录的 `bin/surreal.exe` → 仓根 `bin/surreal.exe`。
   **不回退 PATH**（理由见 §2 最后三行）。
2. **版本闸**：跑一次 `surreal version`，`major != 2` 直接拒并原样抄 `Start-Surreal8009.ps1` 那段错误文案；
   `Cargo.lock` 的 rev 对不上只警告。
3. **起**：`start --user {v_user} --pass {v_password} --bind {v_ip}:{v_port} memory`，
   stdout/stderr 落 `logs/surreal-{port}.log`（`spawn_logged` 同款；**不能继承父进程的控制台**，
   否则 surreal 的启动横幅会插进 gen-model 的启动日志中间）。
4. **等**：`wait_port` 轮询到能连上，默认 30 s；超时把日志尾巴一并带进错误里
   （只说「起不来」而不给它自己的最后几行，等于让人去猜）。
5. **死**：`Drop` + `ctrl_c` 两条路都要杀。**Windows 上光 `Drop` 不够**——父进程被任务管理器强杀时
   `Drop` 不跑，孤儿 surreal 会继续占着端口，而**下一次启动会「成功」地连上那个孤儿**，
   里头装着上一轮的半截数据。这是本计划最难看的失败模式，§3.3 的裁决就是为它准备的。

### 3.3 端口已经被占着，怎么办（决定 Q2）

三种局面，今天的代码一种都分不出来：

| 局面 | 该怎样 |
|---|---|
| 占着的是**上一轮遗留的孤儿 surreal**（署名在，pid 已死） | 最常见。**杀掉重起**（内存库里的东西本来就不该跨进程活着），日志点名「回收了一个孤儿存储进程 pid=N」 |
| 占着的是**本机某个 gen-model 正在用的自家存储**（署名在，pid 活着） | **附着，不杀不拒**。Q5 定案之后这一格从边角变成主路：`spawned-mem` 一旦是默认值，`direct_model_persist` / `db_window_repair` / `world_refno_probe` 这些探针跑起来时**正常局面**就是「服务在跑、库在它手上」——探针要的是读那个库，不是另起一个空库（§3.7） |
| 占着的是**用户自己起的那台生产 SurrealDB**（无署名） | **必须拒绝启动**，不许杀、也不许连——连上去就等于把内存形态的语义悄悄换成落盘形态 |

判别办法：连上去问一句 `INFO FOR ROOT` 分不出后端；可行的是**在自己拉起的那个库里写一枚署名**
（一张 `__gen_model_store` 单行表，装 pid + 启动时刻 + 一个本进程随机 nonce），
起前先连一下读这枚署名：**读得到 + pid 活着 → 附着；读得到 + pid 已死 → 孤儿，杀掉重起；读不到 → 别人的库，拒。**
pid 会被复用，所以 pid 与启动时刻**两者都对上**才算活；对不上宁可拒绝启动。

### 3.4 `/health` 要改口（决定 Q3）

`sul_db.endpoint` 那一栏**被跨部署互踩探测吃着**（`handlers.rs:213-220` 自己写的），
A′ 之后它必须报**真端口**而不是 `embedded:mem`，否则两个 gen-model 抢同一个端口时探测放行。
建议拆两格：

```jsonc
"sul_db": {
  "endpoint": "localhost:8009",        // 永远是真地址；embedded-mem 才回 "embedded:mem"
  "medium":   "spawned-mem",           // external | spawned-mem | embedded-mem
  "durable":  false                    // = medium == "external" 且后端是 rocksdb
}
```

`in_memory_db` 那一栏保留（老客户端在读），值 = `medium != "external"`。

### 3.5 实例锁要拿回来（决定 Q4）

`instance_lock_skip_reason`（`lib.rs:167`）今天在内存形态下**跳过实例锁**，理由是
「两个实例不共享库」。A′ 之后这个理由不成立了：库有端口、别人连得上，两个 gen-model
配同一份 `DbOption.toml` 就是**冲着同一个端口**去的。建议：

- `store_mode = "spawned-mem"` → **照常拿锁**（跳过理由只留给 `embedded-mem`）；
- 锁的说明里那句「`meshes_path` 仍是共享的，先让它分家」原样保留，它跟介质无关。

### 3.6 `PersistTarget` 的介质口径

`PersistMedium` 今天两值（`InMemory` / `RocksDb`），按 `options::in_memory_db()` 判。
A′ 之后 `spawned-mem` 仍然 **不 durable**，所以 `is_durable()` 的答案不变；
但回执上那句「介质=InMemory」会误导人以为是进程内嵌。建议改三值
（`External` / `SpawnedMem` / `EmbeddedMem`），`is_durable()` 只对 `External` 回真。
`bin/e3d_persist_smoke.rs:57` 那句自己接 `mem://` 的代码保留——它是**探针**，
本来就该走 `embedded-mem`，而且 changelog 明确记过「介质由配置说了算，程序不自己挑」这条教训。

### 3.7 三个不经 `run_app` 的入口

`python/src/exec_api.rs:264`、`python/src/lib.rs:91`、`query_service.rs:206` 各自
`init_surreal()`；`bin/direct_model_persist.rs:288`、`bin/db_window_repair.rs:33`、
`bin/world_refno_probe.rs:8` 同样。它们在 A′ 下会连到**还没人拉起来的端口**。两种收法（并入 Q1 一起拍）：

- **A**：把 spawn 收进 `store::ensure_connected()`，全部改调它（子进程按 `OnceLock` 单例）；
- **B**：全部维持现状，文档写明「python / MCP / 探针入口只支持 `external`」。

推荐 **A**——python 的 `full_init` 正是「开箱即用」最可能被人用到的第二条入口。
**但 Q5 定案后 A 有个必答的前提**：`ensure_connected()` 在探针进程里跑起来时，端口上多半已经
有一个服务在用的自家存储，此时它必须**附着**而不是另起（§3.3 第二行）。少了这一格，
默认一翻，每个探针都会在自己的空库上作业、什么都查不到，而且**一声不响**。

### 3.8 默认翻转的到达面（Q5 定案的代价，**P0**）

`store_mode` 默认 `spawned-mem` = **全仓每一份没写这一项的 `DbOption.toml`，意思都变了**。
今天这些文件一律隐含 `external`：

| 文件 | 谁在用 | 翻转后会怎样 |
|---|---|---|
| `DbOption.toml`（仓根） | 日常开发、`run_app` | 正是我们想要的形态 ✅ |
| `DbOption-ams.toml` / `-aba` / `-KMX` / `-zsy` / `DbOption_text.toml` | 各项目现场 | **静默换介质，启动即空库** |
| `db_options/DbOption-l3-suite.toml` | **l3 套件**（`l3_suite.rs:420`） | 套件自己在 8048 上起了一台 rocksdb surreal（`fixture.rs:563-576`），服务却会去 spawn 自己那台 → **整套 e2e 当场连错库** |
| `db_options/DbOption-e2e-8009.toml` / `-issue7-e2e` / `-issue10-e2e` / `-room-live-8029` | 各 e2e / live 现场 | 同上 |
| `db_options/DbOption-init-7997.toml` / `-init-1112` | 初始化夹具 | 灌进一个随进程消失的库，白跑 |
| `db_options/DbOption-rvm-rebuild.toml` | RVM 对拍 | 同上 |

**P0 的活**：给上面除仓根之外的每一份补一行 `store_mode = "external"`，并加一条源码钉护栏
——`db_options/` 下的每个 `.toml` 都必须显式声明 `store_mode`，**新加的配置漏写就红**。
理由很直接：默认值翻转这种事，只在翻的那一天有人记得；护栏是唯一能让半年后新加的夹具也不踩的东西。

另外两处不吃 `DbOption.toml` 的到达面：

- **CI / 部署脚本**里凡是「先起 surreal 再起服务」的段落，服务那一段要带 `AIOS_STORE_MODE=external`；
- **`Start-Surreal8009.ps1` 的存在理由变了**：它从「日常必跑的第一条命令」降为「只有 `external`
  形态才需要」。脚本头部的 `.SYNOPSIS` 要改口，否则 readme 与实际默认互相打脸。

---

## 4. 要拍板的四条

**Q1–Q4 已于 2026-09-04 定案：全按推荐项。** Q5 见下。

| # | 题 | 定案 |
|---|---|---|
| Q1 | 形态开关怎么命名、`in_memory_db` 留不留 | ✅ 三值 `store_mode`，`in_memory_db` 留作别名 + 迁移提示；旁路入口按 §3.7 A 收进 `ensure_connected()`（**这一半随 P3/P4，未做**） |
| Q2 | 端口被占时的裁决 | ✅ 署名判别：活着的自家存储附着、自己的孤儿杀掉重起、别人的库拒绝启动（**P2，未做**） |
| Q3 | `/health` 怎么改口 | ✅ `endpoint` 报真地址 + 新增 `medium` / `durable`，`in_memory_db` 保留兼容（**P4，未做**） |
| Q4 | 内存形态还跳不跳实例锁 | ✅ `spawned-mem` 照常拿锁，跳过只留给 `embedded-mem`（**P3，未做**） |
| Q5 | **默认值** | **已定案（2026-09-04 用户拍板）：默认就是 `spawned-mem`，现有落盘部署自己显式写 `store_mode = "external"`。** 这是一次会波及全仓配置的默认翻转，到达面与迁移步骤见 §3.8——**P0 必须先于 P3 落地**，否则默认一翻，l3 套件与全部探针当场连错库 |

---

## 5. 里程碑

| 步 | 内容 | 依赖 | 产物 / 测试 |
|---|---|---|---|
| **P0** | **默认翻转的迁移（§3.8）**：`db_options/` 与各项目 `DbOption-*.toml` 逐份补 `store_mode = "external"`；CI / 部署脚本补 `AIOS_STORE_MODE=external`；`Start-Surreal8009.ps1` 头部改口 | Q5（已定） | ✅ **2026-09-04 全部落地**（配置 13 份 + `StoreMode` 三值 + 护栏 + 五处「先起库再起服务」的环境变量 + 脚本改口，见下方记录） |
| P1 | `store_process.rs`：解析二进制 + 版本闸 + spawn + `wait_port` + `Drop`/`ctrl_c` 杀 | Q1 | ✅ **2026-09-04 落地**：16 条单测 + 1 条 `#[ignore]` 真起真杀（已手动跑过）。落地形态与本节的差别见下方记录 |
| P2 | 端口署名与**四种**局面的裁决（附着 / 收孤儿 / 拒绝 / 空闲） | Q2 | ✅ **2026-09-04 落地**：13 条判据单测 + 1 条 `#[ignore]` 署名走真库（已手动跑过）。落地形态见下方记录 |
| P3 | 接进 `run_app`：`store_mode` 三分支替换 `lib.rs:1269-1294` 的二分支；**默认值翻成 `spawned-mem`**；横幅改口；实例锁按 Q4 收 | **P0**、P1、P2、Q4 | ✅ **2026-09-04 落地**。**这一步起行为真的变了**，落地形态见下方记录 |
| P4 | `/health` 两格 + `PersistMedium` 三值 + 三个旁路入口收进 `ensure_connected()` | Q3、Q1 | ✅ **2026-09-04 全部落地**（`/health` 三格 + `PersistMedium` 三值 + `ensure_store_connected` 唯一入口 + `StoreStartup::AttachOnly`，见下方记录） |
| P5 | 版本闸规则的**单一来源**：`Start-Surreal8009.ps1` 与 Rust 两份判据不许分叉 | P1 | 源码钉护栏：PS1 里的三条规则（查找顺序 / `major==2` / rev 比对）在 Rust 侧逐条存在 |
| P6 | 实测验收（下） | P1–P4 | ⚠️ **2026-09-04 服务侧全部跑通，并当场撞出一个真缺陷（已修）；plant-ui 端到端未做**（本机没有 plant-ui 的构建产物）。记录见 §8 |

### P6 验收清单（AvevaMarineSample，`watch_dbnums` 先限定到 7997 / 7999）

1. 只起 gen-model 一个进程：日志里出现「拉起存储子进程 pid=N，监听 localhost:8009，介质 memory」，
   `/health.sul_db` 三格正确；`bin/surreal.exe --version` 那一行进日志。
2. 用 `/sql` 探针连 8009，能查到 `pe` 行——**这一格是 A′ 相对进程内嵌形态的全部意义**。
3. 起 plant-ui：**配置一个字不改**，正常连上、树可展开、点眼睛能出三维。
4. `Ctrl+C` 掉 gen-model：8009 立刻不再监听，`tasklist` 里没有残留 surreal。
5. **强杀** gen-model（任务管理器结束进程树之外的方式），确认孤儿存在；再起一次 gen-model：
   日志说「回收了一个孤儿存储进程 pid=N」，库是干净的空库。
6. 先手工起一台 rocksdb 后端的 surreal 占住 8009，再起 gen-model：**拒绝启动**并说清为什么。
7. 冷启动耗时记账：从进程起到 `/health.initialization.startup` 收敛用了多久，
   拆成「拉起子进程 / DESI 基线 / 模型（应为 0，全 lazy）」三段。**这一格决定 §7 那条风险成不成立。**

---

## 6. 新不变量

- **N1 介质由配置说了算，程序不自己挑**（changelog 2026-09-04 已经付过一次学费）。
  spawn 失败不许静默回落到 `external`，`external` 连不上也不许回落到 spawn。
- **N2 内存形态的端口是真的**：`spawned-mem` 下 `/health.sul_db.endpoint` 必须是可连地址，
  不许再出现 `embedded:mem`。
- **N3 子进程不许活过父进程**：正常退出、`Ctrl+C`、panic 三条路都杀；强杀那条靠下次启动的孤儿回收兜。
- **N4 版本闸只有一份判据**：PS1 与 Rust 分叉时以护栏红为准。

---

## 7. 风险与不做

- **「开箱即用」真正的耗时不在起库，在 DESI 基线。** ADR-058 D3 保留了「库首次纳入按
  `1..file_latest` 整库解析」，而内存库**每次开机都是首次**。模型那一半不要紧（D6：`gen_root`
  零行 → 全 lazy，启动一根不排），要紧的是数据那一半。**P6 第 7 格就是为量这个数存在的**；
  量出来若不可接受，下一步是「pe / ATT_* 也改成按需读透」——那是另一份 ADR，不在本计划里。
- **默认翻转会静默改变现有部署的介质**（Q5 已定案接受这个代价）。缓解全在 §3.8 的 P0：
  配置逐份补 `store_mode`、护栏钉住「新加的夹具漏写就红」。**P0 没做完不许动 P3**——
  顺序反了，第一个撞上的是 l3 套件，而它的症状是「测试莫名其妙全查不到数据」，不是一条报错。
- **孤儿回收的署名判别有窗口**：pid 会被复用。署名里除 pid 之外还要存启动时刻，
  两者都对上才认；对不上宁可拒绝启动。
- **`--memory` 后端的内存占用没有量过**：7997 一个库落了 52098 行，全项目是几百个库。
  内存形态天然要求 `watch_dbnums` 限定或足够的物理内存，这一条要写进配置注释，
  不能等人 OOM 之后自己发现。
- **不做**：不改 `mem://` 那条路本身（降级为 `embedded-mem`，探针与单测继续用）；
  不动 ADR-058 的 D3 / D6；不在本计划里碰 plant-ui（它那四条冷库诚实性另有计划）。

---

## 8. P6 实测记录（2026-09-04）

用真的 `aios-database.exe`（release，含 P1–P4）跑的，不是单测。配置是仓根那份换了端口的副本
（`output/out-of-box/DbOption-oob.toml`，**故意不写 `store_mode`**——测的就是新默认），
`AIOS_STARTUP_AUTORUN=0` = 不预解析，`watch_dbnums = [7997]`，`sync_live = false`。

### 跑通的

| # | 事 | 实测 |
|---|---|---|
| 1 | **拒绝别人的库** | 对着被占的 8009 起 → `exit=1`，stderr 原样是「端口上是一台**没有署名**的 SurrealDB…拒绝启动」。真进程真端口 |
| 2 | **自己拉起来** | 换空闲端口 → **启动总耗时 6.47 s**；子进程 `surreal.exe … --bind 127.0.0.1:18022 memory` 在跑 |
| 3 | **`/health` 三栏** | `{endpoint: "localhost:18022", medium: "spawned-mem", durable: false, connected: true}`；横幅按三档说话 |
| 4 | **零预生成** | 启动日志：「gen_root 尚无凭证记录，本次启动不排任何模型工作——从未生成过的根等按需 `ensure`（ADR-058 D6）」 |
| 5 | **树走文件** | `/api/v1/tree/roots` 立刻回 37 个 SITE，`source: "direct"`（冷库、`pe` 0 行也不影响） |
| 6 | **按需生成** | `POST /model/ensure refno=24381/34109`（SITE `/1RX-ARCHHB`）**89.3 s** → `Generated` / `model_available: true` / 416 实例 / 560 生成根 |
| 7 | **落进 kv-mem** | 直连那台库：`inst_relate` **416 行**、`gen_root` 560 行、**416/416 平表三件齐活**（plant-ui 快路一行都不用退回 slim）、**`pe` 0 行** |
| 8 | **署名真的在** | `gen_model/store` 的 `owner:current` 带 `owner_pid` / `child_pid` / `nonce` / 心跳，心跳在动 |

### 撞出的缺陷：**心跳新 ≠ 属主还活着**（已修）

强杀服务后 60 s 内重启，心跳看着还新、写它的人却已经没了 → 此前判「附着」。
**附着的进程不认领这台库、也不接着敲心跳**，它就成了一台没有主人的库；再过 60 s 会被下一个
启动的进程当孤儿杀掉——**而那时它正被人用着**。

实测原样复现：新服务 pid 54872 附着到属主已死（pid 21716）的那台库，416 行旧数据还在，
署名从此没人更新。

**修法**：`judge_port` 里心跳新那一格加一句——属主**明确不在**时照孤儿收（写心跳的那个已经死了，
没有歧义）；查不出属主在不在时仍然附着（新鲜心跳是够强的证据，而这一格的反面是杀进程）。
护栏 `a_fresh_heartbeat_from_a_dead_owner_is_still_an_orphan`。

**修完重跑同一场景**，真机日志变成：

```
*** 回收孤儿存储进程：自家存储的属主 pid=41996 已经不在了（心跳 8 秒前还在写——多半是被强杀，析构没跑），杀掉 pid=44388
*** 存储子进程已就绪：pid=40224 监听 localhost:18022（介质 memory，进程一退整库即消失）
```

### 由此坐实的一条 plant-ui 边界（按码坐实，未实测）

`pe` 是 **0 行**，而 plant-ui 查隐式直管那条子查询取 `in.owner`
（`plant-ui-data/src/lib.rs::query_derived_insts_by_inputs`），没有 `pe` 行就是 `NULL`，
而 `GeomInstQuery.owner` **不是 `Option`** → **一根 BRAN 的直管会整批解不出来**。
非管件走平表快路（`query_insts_flat` 取 `anc[1] as owner_packed`，**全程零解引用**）不受影响。
本轮实测的 SITE 全是 PANE，没有触发这一格。

### 还没做的

- **plant-ui 端到端**：本机 `plant-ui/target/release` 里一个 exe 都没有，没构建过。
  「点树 → 点眼睛 → 三维出来」这一段仍然只有服务侧半条链有证据。
- 上面 §5 P6 清单里的第 3（`/sql` 探针）已顺带验过（本节第 7 行就是拿它查的），
  第 5（Ctrl+C 收尸）**没验**——实测用的是 `Stop-Process -Force`，走的是更坏的那条路。

### 自动化

新增 `scripts/Verify-OutOfBox.ps1`：把能自动判的那半做成一条命令（起服务 / 等就绪 /
打 `sul_db` 三栏 / 树根数 / 收尾查孤儿），剩下一半（点树、点眼睛、看三维）原样列成清单给人。

---

## 9. Q5 翻案：默认翻回 `external`（2026-09-07）

- 拍板：用户「选 B：把代码里的 store_mode 默认翻回 external，连带改那条测试与护栏」。
- **只翻「哪一档是默认」这一格。** 三档形态、`store_process.rs` 的署名裁决与子进程生命周期、
  `/health` 的 `medium` / `durable`、`PersistMedium` 三值、实例锁口径**一个字都没动**；
  `spawned-mem` 与 `embedded-mem` 照旧要得到，只是从此要显式写。

### 撞出来的现场

`test-worklspace/bin/DbOption.toml`（2026-08-25 改的，`store_mode` 与 `in_memory_db` 都没写）
在默认 `spawned-mem` 下被判成「本进程自己拉一台 memory surreal 绑 8009」，而 8009 上蹲着一台
2026-09-05 手工起的 `memory` 后端 surreal，`owner:current` 空表 → `SignatureProbe::Absent`
→ `judge_port` 落 `Refuse` → **拒绝启动**。

这一格谁都没错：那台库不可能有本进程的署名（它不是本进程拉起来的），配置又没说要连它。
**默认值替一份没说话的配置做了一个它不想要的决定**，而症状是起不来。

§3.8 的迁移清单挡住了 `db_options/` 与仓根那 13 份，但它守的是**夹具**——
`test-worklspace/bin/DbOption.toml` 与仓根 `DbOption.toml` 都不在护栏覆盖范围里，
而现场配置恰恰是人真拿来跑的那一份。

### 改了什么

| 位置 | 改动 |
|---|---|
| `options.rs::effective_store_mode` | `Some(false) => External` / `None => SpawnedMem` 合并成 `Some(false) \| None => External` |
| `options.rs::store_mode` 文档注释 | 默认口径改写，点名「`spawned-mem` 默认下 `judge_port` 只能判分不清并拒绝启动」 |
| 测试 `the_default_store_mode_is_spawned_mem` | 改名 `the_default_store_mode_is_external`，并加一句「显式写 `spawned-mem` 仍然要得到」 |
| 护栏 `every_db_option_fixture_declares_its_store_mode_and_data_face` | **判据一个字没动**——它本来就与默认指向哪一档无关；只把文档注释与失败信息改成「默认翻过两次，不许有配置去依赖它」 |
| 13 份夹具 / 现场配置的 `store_mode` 注释 | 「默认值**将**翻成 spawned-mem」改成「翻过两次，翻过的默认不能被依赖」 |
| `Start-Surreal8009.ps1` | `.SYNOPSIS` / `.DESCRIPTION` 改回「external（默认）要先跑它；`spawned-mem` 那一档才不要跑」 |
| `Start-AiosDatabaseManual.ps1`、`Run-LiveBatch.ps1`、`Run-Issue7E2E.ps1`、`Run-RoomE3DE2E.ps1` | `AIOS_STORE_MODE=external` **保留**（不靠默认本来就是对的），只改那句已经反了的理由 |
| `python/src/lib.rs`、`python/src/exec_api.rs` | 「只附着不拉起」的理由从「默认已是 spawned-mem」改成「不管默认指向哪一档」 |

### 实测：同一份配置、同一个端口，只差一个环境变量（2026-09-07）

在一个隔离运行目录里复现 §9 开头那个现场——配置取 `test-worklspace/bin/DbOption.toml`，
**删掉 `store_mode` 那一行**（`in_memory_db` 本来就没有），`v_port` 指向一台现起的、
**没有署名**的 memory surreal（`owner:current` 空表，与 8009 那台同形）：

| 跑 | 环境变量 | 结果 |
|---|---|---|
| A | 无（= 新默认 `external`） | `数据库已经连接到 AvevaMarineSample, 站点: ws://127.0.0.1:18777`，启动序列走完（surql 灌入、水位播种、索引就绪、平表清扫、watcher 起） |
| B | `AIOS_STORE_MODE=spawned-mem`（= 翻转前的默认） | `Error: 端口上是一台**没有署名**的 SurrealDB……所以拒绝启动。` |

**两跑之间只差那一个环境变量。** B 逐字复现了 2026-09-07 16:25 那次起不来，A 证明翻回默认之后
同一份没写形态键的配置直接过门；同时 B 也说明**署名裁决本身一点没松**——它仍然拦得住
「配了自带存储、端口上却蹲着别人的库」，只是不再由默认值把所有沉默的配置推进这一格。

`AIOS_STARTUP_AUTORUN=0`，没有碰 8010 那台真库，也没有动机器上任何在跑的服务。

**随后在真环境上又验了一次**（`test-worklspace/bin`，8010 那台 880 万行 `pe` 的 rocksdb 库）：
换上新二进制、把 `store_mode = "external"` 那一行**注释掉**（`data_face` 本来就没写、无任何
`AIOS_*` 覆盖）后照常起服务，16.53 s 起完，横幅与 `/health` 逐格对上：

```
数据库已经连接到 AvevaMarineSample, 站点: ws://127.0.0.1:8010
  存储介质：ws://127.0.0.1:8010
  数据面：ingest（整库基线 + 稳态增量窗口，推进 applied_sesno）

/health  sul_db = {"connected":true,"endpoint":"127.0.0.1:8010","medium":"external","durable":true}
         data_face = "ingest"   in_memory_db = false
```

`data_face` 那一格是白拿的连带证据：配置一个字没说，它随 `external` 派生成了 `ingest`，
而不是翻转前的 `read-through`。空间树指纹一致、跳过全量房间重建、`dbnum=8000` 一致到 379，
与换二进制前那个进程的状态接得上。

### 连带效果：`data_face` 跟着翻

`data_face` 是从 `store_mode` 派生的（`data_face_for_store`），所以没写形态键的配置，
数据面同时从 `read-through` 变回 `ingest`。这正是护栏逼着**两个键**都显式写的理由。

### 未做 / 在案

- `docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` 整份以「kv-mem 是默认形态」
  为前提，标题与 §3 若干处今天不成立。**没改**——那份计划的技术内容（零摄入、写透）不受影响，
  受影响的只有「默认」二字。
- 13 份配置里 `data_face` 那段注释仍写着「默认值是 read-through」与「环境变量 AIOS_DATA_FACE
  压过本键」，两句今天都不对：前者随本次翻转变成 `ingest`，后者在 `data_face` 改成派生量之后
  就不成立了（环境变量只剩报警作用）。**没改**，是单独一件事。
- 仓根 `DbOption.toml` 仍不写形态键（护栏放过它）。默认翻回 `external` 之后，它 `:101` 那句
  「默认不写 = 连上面那台 SurrealDB」**又对了**，无需改。
