# 自动升级 · 协作板（只追加，不改别人的行）

- 设计：`UPGRADE-DESIGN.md`（同目录，约定都在里面；有出入先写到下面「要协调者定」）。
- 计划：`UPGRADE-PLAN.md`（同目录，用户批准的那份的拷贝，只读）。
- 分工和验收：设计第 10 节。老板 `BOARD.md` 不再写升级的事。

## 规矩

- **编译一律经编译锁**，做法和以前一样（`..\spike\HANDOFF-DASHBOARD.md`、老板「规矩」第一条）：`-Who <代号>`、`-Jobs 2 -StartCommitFreeGb 14`、`--offline`。三位共用 `spike\target`，编之前在「正在编译」记一行，编完划掉。碰到 90 / 91 重排即可。
- **树要一直能编。** 新模块先自己编过再接进来；别人的编译错不要去修，写到「要别人配合」。
- **依赖只用 `spike\Cargo.lock` 里已有的包**，不联网、不装东西。
- **没有 git。** 开工前的底稿在 `remote\baseline\20261113-0238-upgrade\`（三个包的全部源码加 `Cargo.toml`、`Cargo.lock`，`MANIFEST.txt` 带 SHA256）。要看自己改了什么就和它比。
  - **2026-11-13 09:0x 更新：有 git 了，而且换了工作区。** 新 worktree：`D:\work\plant-code\old\deploy-dash-upgrade`（分支 `feat/deploy-upgrade`，从 `output\gui-20261112` 的 `f55a7aa` 拉出）。`git status` / `git diff` 直接看；上面那条「没有 git、和 baseline 比」作废。**`output\gui-20261112` 那棵树不要在这条线上改**（用户的面板正在用它的 exe）。
- **所有监听只绑 127.0.0.1。** 不建防火墙规则、计划任务、服务。
- **不打印、不写入口令、令牌、升级密钥、真实地址**：板、文档、汇报、截图、测试夹具、日志样例里都用占位符。
- **磁盘**：产物、沙盒、临时文件都放 D 盘；C 盘低于 2.5 GB 就停手并在板上说。
- 每做完一步在「进度」记一行：`时刻 代号 做了什么 / 卡在哪`。宿主可能重启，做完一步就落盘。
- 收工时写 `remote\<代号>\REPORT-upgrade.md`。

## 不许碰的

- **用户开着的面板**（`dash\run-aps\deploy-dash-final.exe`，真实模式，选中的是「云服务器」）、**它的本机代理**（听 127.0.0.1:8600，根是 `dash\aps-test2\pkg`）、**ssh 隧道**（本机 9099 / 28600 / 28900 / 28922 / 28998）：不结束、不重启、不占它们的端口。
- **真实模式的面板全机只能开一个**，现在被用户那个占着。我们的测试面板一律用 mock 模式开，要真服务就让它连「远程」的代理（不带界面库的代理接真实后端，根是我们自己的沙盒）。
- **用户的面板存档** `%LOCALAPPDATA%\AiosDeployDash\`（里面有「云服务器」那台）：不改、不往外读令牌。测试一律用另外的存档目录。
- `dash\aps-test2\pkg`：现在是用户面板里的「本机」，不在里面部署、不动。
- `dash\run-aps\` 里已有的 exe：不覆盖、不往里放新的。交付的新 exe 放新目录 `dash\run-up\`：面板 `deploy-dash.exe`，旁边是配套的代理 `deploy-dash-agent.exe`（设计 7.1 按这个名字找它）。
- **用户可能正在用这台电脑**：不抢前台、不动鼠标键盘。用 agent-desktop 时只操作自己起的那个面板窗口（按进程号认），不要点到用户那个面板上。
- **云服务器**：U7 之前谁都不连。U7 由 agent-client 做，等协调者在本板发话。
- 本机别人的服务和端口（8023 / 8029 / 3101、3080、8199 等，清单见 `deploy-agent\src\config.rs` 的 `RESERVED_PORTS`）：不碰。8023 那套在跑别人的增量测试。
- `scripts\stack\*` 属于主线：只调用、不改。gen-model 仓库里受 git 管的文件都不改。

## 目录和端口（各用各的，都只听本机）

| 谁 | 目录 | 端口 |
|---|---|---|
| `agent-server` | 小单测用系统临时目录；大文件的测试（几十 MB 以上）和手工冒烟一律放 `spike\tmp\up-server\`（C 盘紧，不要往系统临时目录写大文件），测完删 | 代理 18631–18639 |
| `agent-client` | 沙盒甲 `dash\up-a\pkg`、沙盒乙 `dash\up-b\pkg`；存档 `remote\agent-client\up-store\` | 甲：19109 库 / 19122 后端 / 19198 rs-server / 19100 网页，代理 18641；乙：19209 / 19222 / 19298 / 19200，代理 18642；mock 代理 18643–18649 |
| `fleet-ui` | 存档 `remote\fleet-ui\up-store\`；截图 `remote\shots\upgrade\` | mock 代理 18651–18659 |
| `m4d7qv2s`（P0 验收、搬到 tools） | 沙盒 `spike\tmp\up-p0\`、`spike\tmp\tools-port\`；新 exe `dash\run-up\`（不进库） | mock 代理 18691–18699；`tools-port` 的端到端用脚本缺省的 18681/18682 |

沙盒是自己建的，用完可以连根删；删之前确认里面的服务和代理都停了。

## 里程碑

（协议类型可编、数据层骨架可编、U1 … U7 各自验收过，在此追加）

- 02:41 **【协调者】开工。** 设计定稿（`UPGRADE-DESIGN.md`），底稿留好。agent-server 先交「协议类型可编」；agent-client 先把「只看」驱动关到特性开关后面，再交数据层骨架和 `agent-client\UPGRADE-API.md`；fleet-ui 先做存档两项和不依赖数据层的界面，等骨架那一行再接。
- 08:2x **【接手】换工作区，单会话推进。** 用户要求开新 worktree 实现这个功能：新树 `D:\work\plant-code\old\deploy-dash-upgrade`（分支 `feat/deploy-upgrade`）。原三位代理 02:56 起停滞、本轮不续；由主会话自己按 U0 → U1 → U2 … 推进，每步编译 + 测试 + 提交。
- 08:35 **【U0 ✓】协议类型可编。** `deploy-dash-proto\src\upgrade.rs`（现状/随机数/会话与清单/分块内容/升级报告/历史，13 个升级错误码，签名文本拼法，8 MiB 分块上限，`upgrade.v1` 能力）；`JobKindName` 加 `upgrade`/`rollback`/`upgrade_agent` + `is_upgrade()`；`JobArgs.upgrade`、`JobResult.upgrade`（注释写明和 `BackendJobs::upgrade` 那个库架构升级进度不是一回事）；11 份夹具。提交 `ce070d8`。`cargo test -p deploy-dash-proto -p deploy-agent` 全过（proto 5+14、agent 47+13），`cargo check --workspace` 通过。
- 09:0x **【U1 ✓】传输通了。** 代理侧：`upgrade_key.rs`（密钥/指纹/HMAC/随机数池）、`upgrade_store.rs`（5.1 路径把关、5.2 判定表、5.3 停不停、内容库续传与核 sha）、`upgrade_http.rs`（六个端点）、`http.rs` 按路径给请求体上限（只有 blobs 8 MiB，且读完头才判）、`/v1/info` 加 `upgrade.v1`、`agent.toml` 加 `[upgrade]`、无界面代理加 `upgrade-key` 子命令。提交 `af6890f`。`cargo test -p deploy-agent` 106 个全过、零警告（79 单测 + 12 端到端 + 15 既有）。`upgrade-key` 的生成/重复拒绝/--id/--rotate/--disable/互斥开关手工跑过。

## 正在编译 / 占用

（谁在编、从几点起；编完划掉）

- **真实模式占用 · 用户**：用户的面板开着，别人不要起真实模式的面板。
- ~~02:56 agent-client 经锁编 `deploy-dash-agent`~~ 08:3x 起改用新 worktree 的锁记录，这一行作废（原进程 34852 早已退出，锁文件已清）。
- ~~16:37 起 **m4d7qv2s**（P0）经锁跑 `cargo test --offline -p deploy-dash-proto -p deploy-agent -p deploy-dash`（合并 main `736a41f` + 安全修复 `0533cb7`），排在 room 后面。~~ 16:47 编完（测试一轮 + 单独编两个 exe），之后没再编。
- ~~17:2x 起 **m4d7qv2s**（把升级线搬到 `tools\deploy-dash`）经锁跑 `cargo test --offline -p deploy-dash-proto -p deploy-agent -p deploy-dash`（stable，`--manifest-path tools\deploy-dash\Cargo.toml`，`--target-dir output\gui-20261112\spike\target`），之后还要编 `deploy-dash-agent` 和 `deploy-dash` 两个 exe。~~ 17:23 拿到锁，17:28 测试跑完，17:29 两个 exe 编完，之后没再编。这台 cargo 不认 `--target-dir`：成品落在 `tools\deploy-dash\target`（那边的 `.gitignore` 已忽略），中间产物在 `D:\Rust\.cargo\targets\gen-model-6965b07b`，`output\gui-20261112` 里没写东西。

## 协调者说

（我验收时发现的问题、对设计的改动，写在这里。每做完一步回来看一眼，有点到你的就先办）

- 02:50 **【给 agent-server】取消。** 设计 5.4 第 3 步写了「等后端空下来的这段时间任务可以取消」，而现在 `POST /v1/jobs/<id>/cancel` 一律回 501。定：**只有升级类任务、只在「等后端空下来」这一段**能取消（取消后把增量同步恢复成原样，任务状态 `cancelled`，什么都没动）；别的任务、别的阶段照旧 501。取消只要令牌，不要签名（它只会让事情不发生）。
- 02:50 **【给 agent-server、agent-client】任务号是整数。** 现有协议里任务号是整数（`job_id: 17`），设计 4.2 的 `busy.job_id`、4.6 报告里没写任务号的地方都照现有的来；会话号、应用号（`0003-u2026…`）是字符串。时刻一律照现有约定（ISO 8601 带时区）；只有 `release.created_at` 是照抄 `release.json` 里的原文。
- 02:50 **【给三位】我会自己写一个不依赖你们代码的探针**（照设计文档写的 Python 脚本，`remote\coord\upgrade_probe.py`），对着 mock 代理把协议、验签、路径把关、续传、换回从头到尾打一遍。它用端口 18661–18669、目录 `spike\tmp\up-coord\`。探针和实现对不上的地方我会写在这里：是设计没写清的我改设计，是实现偏了的请改实现。
- 09:0x **【U1 时定下的三处，实现和设计有出入，记在这里】**（都是设计没写死、按代码现实定的）
  1. **`upgrade` 这个名字已经被占了。** `status::BackendJobs::upgrade` 是**库架构升级**的进度（`UpgradeProgress { done, total }`），和程序升级无关。设计 4.6 要在 `result.upgrade` 里放升级报告——同名不同物。实现照设计用了 `JobResult.upgrade`（在任务结果里），但两处注释都写明不要互相覆盖；如果后面觉得容易混，把报告改成 `upgrade_report` 只需要改 proto 一处 + 两个夹具。
  2. **随机数池 16 → 32。** 设计 4.3 写「同时最多留 16 个」，但面板并行传两台、每台 manifest + job + 重试，很容易把自己挤掉。实现用 32。
  3. **`offset` 对不上要回 409，而原实现是「回 200 + 当前 have」。** 端到端测试抓到：设计 4.5 明确写「否则回 409 `offset_mismatch`，体里带 `have`」，实现一开始先写后判，已改成写前就比。**这条是照着设计改的实现，不是改设计。**
  4. **三个升级类 kind 现在回 400 而不是 501。** 设计没说这一版怎么回；实现选了 400 + 一句「U2 还没接上」，因为 501 会被面板当成「这个端点不支持」而不显示原因。U2 接上时这一行会改。
     **U2 已接上（`dc1e336`），这条作废。**
- 10:2x **【U3 时发现的两处，都是实现偏了、已改】**（不是改设计）
  1. **`offset` 对不上要回 409。** U1 时记的第 3 条，端到端又验了一遍：续传路径正常。
  2. **换文件中途失败也要落历史。** 设计 4.6 说「每次升级都往 `history.jsonl` 追一行」，
     实现只在 `rollback_and_report` 那条路上写了，`swap` 失败那条路漏了。已补。
     这是「设计对、实现漏」的例子，端到端跑出来的。

## 要协调者定

（设计里写得不对、实现不了、两条打架的，写在这里；我定了会改设计并在「协调者说」回一句。不耽误别的活的，先按你认为对的做、在这里记下你是怎么做的）

## 要别人配合

## 进度

- 08:1x **【协调者/Devin】断点记录 + 工作区入库。** 三位代理会话已结束（最后活动 02:56），升级线停滞约 5 小时未交付；现状与续派顺序见 `UPGRADE-RESUME-20261113.md`。工作区现已建 git（首提 `383e796`），「没有 git、比 baseline」那条规矩作废，diff 用 `git status`/`git diff`，baseline 目录留作对照。
- 08:1x **【协调者/Devin】agent-client 第一步核实完毕。** `watch-driver` 特性已编入且能编过（`up\logs\w1-agent-watch.log.stderr` Finished 5.86s）；`spike\target\debug\deploy-dash-agent.exe`（02:43）实测为缺省特性版、不认 `--watch`，符合「缺省关」设计。02:56「正在编译」一行可视为已结束。
- 08:25 **【接手】开新 worktree。** `D:\work\plant-code\old\deploy-dash-upgrade`，分支 `feat/deploy-upgrade`，从 `f55a7aa` 拉出。共享 `targets\gui-20261112-b5fba78a`（huacnlee cargo 按工作区名分目录，两个树共用一份依赖，所以第一次 `cargo test -p deploy-dash-proto -p deploy-agent` 只要 22 秒）。顺手清掉 `output\ops\build.lock` 里 02:43 留下的死锁（持锁进程 34852 早已退出）。
- 08:35 **【U0 ✓】提交 `ce070d8`。** proto 加 `upgrade.rs` + 11 夹具 + 三个任务种类；agent/panel 侧只做接线（label、confirm、`is_upgrade`、`plan()` 回「U2 还没接上」）。测试全过、`cargo check --workspace` 过。
- 09:0x **【U1 ✓】提交 `af6890f`。** 代理侧传输全通：密钥与验签、会话与清单、分块传内容（可续传、按 sha256 核）、路径把关、判定表、内容库。`cargo test -p deploy-agent` 106 全过零警告。`upgrade-key` 子命令手工验过六种用法。
- 09:0x **【U1 的一处实现修正】** 端到端测试抓到 `offset` 对不上时先写后判（回 200 而不是设计要求的 409），已改成写前就比。这是测试起作用的例子，记在「协调者说」第 3 条。

## 下一步（U2 的入口，先拍板再动手）

U2「应用与换回」有一个设计没写、代码现实卡住的地方：**代理进程里没有起进程的能力**。
`deploy-agent` 全仓没有一处调 `stack.ps1`（只有 `install.rs` 跑 netsh/schtasks、`token.rs` 跑 whoami）；
真正跑 `stack.ps1 up -NoInit` / `down` 的代码在 `deploy-dash\src\data\stack.rs:681`（`StackCliDriver::run_job`），
而 `deploy-dash` 依赖 `deploy-agent`，反过来不行。

所以 U2 要拆成两半，**归属必须先定**：

- **代理侧（`deploy-agent`）**：状态机（等空闲 → 停 → 换文件 → 起 → 检查 → 失败换回）、`journal.jsonl`、
  崩溃恢复、备份轮转、报告与历史。这些不需要起进程，只改文件。
- **执行侧（`deploy-dash`）**：新增 `BackendCommand::Up`/`Down` 之外的「按计划停、换、起、查」编排，
  或者把 `StackCliDriver` 挪进 `deploy-agent`（会引入 `deploy-agent → deploy-dash` 的反向依赖，不行），
  或者让代理直接调 `stack.ps1`（需要 `deploy-agent` 自己拼命令行，`stackcfg.rs` 已有读 `stack.toml` 的基础）。

三个候选做法，推荐第 3 个（代价最小、不破依赖方向）：

1. 把 `StackCliDriver` 从 `deploy-dash` 挪到新 crate `deploy-stack-cli`，两边都依赖它；
2. 让 `deploy-dash` 暴露一个 trait，代理在面板里跑时用它、headless 时没有（等于自更新场景做不了）；
3. **`deploy-agent` 自己起 `powershell.exe -File stack.ps1 up -NoInit`**（照 `stack.rs:688` 那段抄，
   包括 `-NoProfile -NonInteractive -ExecutionPolicy Bypass`、`env_remove("PSModulePath")`、`CREATE_NO_WINDOW`）。

**09:5x 已按第 3 个做法实现**（见下面 U2 ✓），不需要再等点头。

## 进度（续）

- 09:2x **【U2a ✓】`stack_cli.rs`。** 代理自己跑 `stack.ps1`：参数照面板那份、`PSModulePath` 去掉、
  `CREATE_NO_WINDOW` + 申请脱离 Job、stdout/stderr 各一线程按到达顺序收、逐行回调。
  单测用一个假 `stack.ps1`（认 `-Config`、`down` 回 6）验退出码与行都如实回来。
- 09:4x **【U2b ✓】`upgrade_apply.rs`。** journal 先落盘再动手（fsync）；place/backup/conflict/release_json
  四种操作；照日志**倒着**换回，可重复做结果一样；半行日志跳过；备份轮转；历史 jsonl。
  15 个单测覆盖设计 5.5/5.6/5.8 的每一行，包括「升级之后又被手改过的文件不误收进内容库」。
- 09:5x **【U2c ✓】`upgrade_run.rs`。** 状态机照设计 5.4 的 1–8 步；preflight 服务不正常就不动手；
  等空闲超时把增量同步恢复原样；down/up 非 0 走换回；检查要「连续两份快照都健康」且库版本对得上；
  库版本对不上且没 `allow_storage_rebuild` 就换回（`failed_step=storage`）。
  崩溃恢复扫 `applied\*\journal.jsonl`，没结果的照日志换回并写报告。
- 09:5x **【U2d ✓】接进 `POST /v1/jobs`。** 三个升级类 kind 要签名（用途 `job`），会话必须 `ready`；
  不进后端命令链，由代理建本地任务（`JobTable::create_local`）在后台线程跑，结果带 `result.upgrade` 报告。
- 09:5x **【U2 ✓】提交 `dc1e336`。** `cargo test -p deploy-dash-proto -p deploy-agent` 156 个全过。
  两个真起代理的端到端：① 升级成功（文件换了、exit 0、历史有、`can_rollback` 变真）；
  ② 新版起不来（`up` 回 4）→ 自动换回、文件回旧版、`failed_step=up`。

## 下一步（U3 / U5 二选一）

- **U3 面板数据层与命令行**：读来源包、算差、上传（重试与续传）、逐台应用、`deploy-dash.exe upgrade …`。
  依赖 U1 的端点（已通），不依赖 U5。
- **U5 代理自更新**：`upgrade_agent` 现在明确回「这一版还没接上」（exit 2）。要做的话：
  试跑新 exe → 改名替换 → `--takeover` 交接端口。唯一要动既有启动路径的地方是
  `sys::acquire_single_instance` 的语义（现在只有「拿到」和「已被占用 → exit 2」两种），
  要加第三种「带超时轮询获取」。

建议先做 U3：面板能用起来才看得出整套对不对；U5 只影响代理自己换版本这一条路。

## 进度（U3）

- 10:0x **【U3a ✓】`data/upgrade.rs`（~870 行）。** 来源包 `Source`（读 `release.json`、按 sha256 核、
  按 sha 去重、`wanted()` 列出要传的）、`Target`（令牌 + 升级密钥 + 地址 + `headers()`）、
  `Reply`（Ok/Denied/Refused/Down，把代理的各种 4xx 分开）、`call` / `call_bytes` / `get_json`、
  `Preview` + `preview()`（算差、给 `blocked` 原因、判断会不会动网页）、`Transfer` + `transfer()`
  （开会话 → 签名交清单 → 分块传，409 按 `have` 续传）、`Applied` + `apply()`（下 upgrade 任务、
  轮询 `/v1/jobs/<id>`、`on_step` 报进度）、`ApplyOptions`、`Summary`（`any_failed`/`any_skipped`/
  `exit_code` 0/1/3/`to_json`）、`run_round()`（预览 → 传 → 一台一台应用，一台没成就停、canary 停）、
  `Event` 枚举。9 个单测。
- 10:0x **【U3b ✓】`upgrade_cli.rs`（~370 行）。** `deploy-dash.exe upgrade --source <目录>
  (--all | --servers …) [--canary] [--yes] [--json] [--no-agent] [--allow-storage-rebuild]
  [--store <目录>] [--wait-idle <秒>]`；`main()` 开头分派，不挂窗口。退出码照设计 7.5：
  0 都成 / 1 有没成功的 / 3 没有失败但有跳过的 / 2 参数或前置条件不对。4 个单测。
- 10:0x **【U3c ✓】两处底层补齐。** `http.rs` 加 `request_bytes_to`（收 `Option<&[u8]>`，
  `request_to` 转调它）——原来的 body 只有 `&str`，传不了二进制；`client_store.rs` 的
  `ServerEntry` 加 `upgrade_key`（DPAPI，**独立 entropy**，和令牌不共用）和 `auto_upgrade`，
  老存档没有这两项照读。
- 10:1x **【U3 修一处真缺陷】** 端到端跑到「第一台换文件失败」时发现：`upgrade_run.rs` 里
  `swap` 失败那条路（换文件中途失败）**不写 `history.jsonl`**，只有 `up` / 检查 / 存储失败那几条
  走 `rollback_and_report` 才写。结果是这条失败在面板的「上一次结果」里根本不存在。
  补上写历史（带 `from` / `to` / `rollback`），顺手把 `rollback_and_report` 里写历史失败从
  `let _ =` 改成记一行。新增单测 `swap_failure_rolls_back_and_still_writes_history`。
- 10:2x **【U3 ✓】提交 `0190a77`。** 测试：deploy-dash **84 过**（含新增的二进制 body 往返、
  来源包核对、预览阻塞、签名、汇总退出码），deploy-agent **138 过**（109 lib + 2 dev bin +
  13 http_api + 14 upgrade_api）。

**端到端（`remote/upgrade-e2e.ps1`）**：真 `deploy-dash-agent.exe` ×2 + 假 `stack.ps1` + 两个沙盒 +
真 `servers.toml`（DPAPI 加密，用面板自己那套 entropy）+ 真来源包，跑真命令行。

| 场景 | 做什么 | 结果 |
|---|---|---|
| `-Scene all` | 两台都能升 | 两台都换成新内容、退出码 0、两台都有 `history.jsonl`、报告 `outcome=ok`、`files.replaced=1` |
| `-Scene fail-first` | 独占打开第一台的目标文件，让「挪旧文件进备份」失败 | 自动换回、旧文件回来、日志 `begin → backup → rollback_begin → rolled_back`、历史 `outcome=rolled_back failed_step=swap`、**第二台一次都没碰**（连传都没传，无历史）、退出码 1 |

一处要说明：**沙盒里注入不到「`up` 起不来」**。`upgrade_run.rs` 只在
`needs_stop && services_running` 时才调 `stack.ps1 up`，而沙盒里没有在跑的四个服务，
所以假 `stack.ps1` 的 `up` 分支根本不会被调。真机上「起不来 → 换回」这条路由
`upgrade_run.rs` 的单测 `up_failing_rolls_back_and_restores_services`（`up` 回 4）覆盖；
端到端这边改在「换文件」这一步注入失败，走的是另一条换回路径（5.6 照日志换回）。

## 下一步（U4）

- **U4 界面**：升级抽屉（预览、勾选、进度、结果、回到上一版、重新摆包）、服务器行版本徽标。
  数据层已经全在了（`data/upgrade.rs` 的 `run_round` 按 `Event` 回报进度，界面直接接），
  `ServerEntry.auto_upgrade` 和 `ApplyOptions.parallel_uploads` 是给这一层留的字段（现在
  带 `#[allow(dead_code)]`）。
- U5 / U6 / U7 见上面「下一步（U3 / U5 二选一）」。U7 仍等协调者发话（云服务器 SSH 被爆破
  那件事还没处置）。

## 进度（P0 · 合并 main + 安全修复）

- 16:34 **【P0a ✓】合并 main，提交 `736a41f`。** 只解冲突，不夹带别的改动：`config.rs` 字段取并集
  （main 的 `no_auth` + 本线的 `[upgrade]`）；`server.rs` 的引用、`Shared`、`allowed()` 取并集；无界面代理
  同时认 `--no-auth` 和 `upgrade-key`。请求体上限按本线的设计 4.5 走（缺省 1 MiB、升级 blobs 8 MiB），main 原来
  一律 8 MiB；网页老的推程序接口 `PUT /v1/services/upload` 按 2 MiB 一块传原始字节，单开 4 MiB。
- 16:37 **【P0b ✓】安全修复，提交 `0533cb7`。** ① 免令牌模式只放行 GET，写接口（任务、配置、推程序、升级）照旧
  要令牌；② 跨源应答头原来每个应答都带 `*`，改成只在令牌模式、请求带 `Origin` 时给，`agent.toml` 新增
  `cors_origins`（空 = 任何来源，请求照样要令牌），免令牌模式一律不给，放行头补上 `X-Upgrade-*`；③ 老的推程序
  接口（`services/upload`、`stage-clear`、`upgrade`、`rollback`）在服务器上没有升级密钥时一律 403
  `upgrade_disabled` 并记审计。新增 3 个集成测试，另补 `/v1/jobs` 超 1 MiB 回 413。
- 16:43 **【P0 测试 ✓】** 经锁 `cargo test --offline -p deploy-dash-proto -p deploy-agent -p deploy-dash`：
  **247 过 / 1 忽略 / 0 失败**（agent 库 112、dev bin 2、http_api 16、upgrade_api 14、面板 84、proto 5 + 夹具 14），
  警告只有原来就有的未用字段/方法。16:47 经锁单独编 `deploy-dash-agent`（`--features headless-agent`，普通
  `cargo test` 不编它），没有新警告。
- 16:48 **【P0 端到端 ✓】** `upgrade-e2e.ps1 -Scene all`、`-Scene fail-first` 合并后照旧全过，结果同「进度（U3）」的表。
- 16:59 **【P0 网页验收 ✓】** mock 代理（`dash\run-up\deploy-dash-agent.exe --mock`，127.0.0.1:18691），走 HTTP
  （内置浏览器这台用不了）：首页不用令牌能打开，`ping`/`info`/状态能列，没带令牌的写 401，跨源和预检头对；
  切「运行中」场景后单停 `rs_server` 只它停、再单起回到 ok；没开升级时老推程序接口 403，开了以后照网页的做法
  推 3 MiB（2 MiB + 1 MiB 两块）收下、版本对比里看得到暂存，一块 5 MiB 回 413。mock 代理已停，18691 已放。
  详见 `remote\m4d7qv2s\REPORT-upgrade.md`。

## 进度（P0 续 · 搬到 `tools\deploy-dash`）

另一窗口 15:18 已把三个 crate 从 `output\gui-20261112\spike\` 搬到 gen-model 的 `tools\deploy-dash\`（未提交），
快进 main 没有意义了；用户定「现在就搬（先备份）」。

- 17:20 **【备份 ✓】** 搬之前整个 `tools\deploy-dash`（不含 `target`）打包到 gen-model 的
  `_scratch\deploy-upgrade-plan\p0\backup\tools-deploy-dash-before-port-1720.zip`（92 个文件），旁边的
  `.manifest.csv` 记了每个文件的大小、sha256、修改时间。
- 17:21 **【搬 ✓】** `main..feat/deploy-upgrade` 里三个 crate 的差异（41 个文件：新增 21、修改 20，不含两个
  `Cargo.toml`）打到 `tools\deploy-dash`，逐个和分支对过。两个 `Cargo.toml` 手改：`deploy-agent` 加 `hmac`；
  `deploy-dash` 加可选的 `sha2`、`hmac`，挂到 `remote` feature。`Cargo.lock` 只多 3 行依赖边，没联网。
  `dashboard.html`、`PROTOCOL.md` 本线相对 main 没改，保持另一窗口的版本。gen-model 里没提交。
- 17:28 **【测试 ✓】** 经锁、stable、`--manifest-path tools\deploy-dash\Cargo.toml`：**247 过 / 1 忽略 / 0 失败**，
  各项数目和 P0 一样。17:29 经锁编 `deploy-dash` 和 `deploy-dash-agent`（`--features headless-agent`）。
- 17:31 **【端到端 ✓】** 两个 exe 拷到沙盒 `spike\tmp\tools-port\`，`upgrade-e2e.ps1 -Root <沙盒>` 的
  `-Scene all`、`-Scene fail-first` 都过。
- 17:33 **【网页验收 ✓】** tools 版代理（从沙盒副本跑，带另一窗口新改的 `dashboard.html`）mock 跑在
  127.0.0.1:18691，P0 那三套检查全过；开升级密钥不用重启代理。代理已停，18691 已放。
- **给编 tools 的人：** 这台 cargo 不认 `--target-dir`（只给一句警告），成品总落在 `tools\deploy-dash\target\`，
  中间产物在 `D:\Rust\.cargo\targets\gen-model-6965b07b\`（已编好，之后增量）。`tools\deploy-dash\Cargo.toml`
  开头那条「`--target-dir` 指向已经编好全部依赖的那份缓存」在这台机器上不起作用。
  详见 `remote\m4d7qv2s\REPORT-upgrade.md` 末节。
- 17:59 **【清理 ✓】** 用户同意后删了临时沙盒 `dash\run-up\`、`spike\tmp\up-p0\`、`spike\tmp\tools-port\`（删前确认没有进程在用）。`spike\tmp\up-e2e\`（`upgrade-e2e.ps1` 缺省的沙盒）没删。
