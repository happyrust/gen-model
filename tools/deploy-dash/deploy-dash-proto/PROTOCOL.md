# 部署面板 ↔ stack 代理 · 线上协议（`schema = 1`）

- 依据：`output\gui-20261112\REMOTE-ACCESS-DESIGN.md` 第 4 节（2026-11-12 20:08 批准，第 10 节 14 条全按推荐）。本文是正式版，**和设计有出入的地方以本文为准**，出入逐条列在第 9 节，正文里用「〔出入〕」标出。
- 类型：`tools\deploy-dash\deploy-dash-proto\src\*.rs`（Rust 名字就是本文的名字）。服务端实现：`tools\deploy-dash\deploy-agent`。
- 样例：每个端点一份 `tests\fixtures\*.json`，也以常量暴露在 `deploy_dash_proto::fixtures::*`；单测把每份解析成类型再序列化回去，必须逐字段相等（`tests\fixtures.rs`）。
- 规矩：**只加字段不改版本**；改含义、删字段才把 `schema` 加一。2026-11-12 20:40 起冻结（类型名和字段只加不改）。
- 22:0x 只加（`schema` 仍是 1）：摘要的 `stack` 多 `first_run`、`lock_command`（3.3）；模拟数据的 `/v1/ping` 能冒充别的协议版本（6.7）；夹具多一份 `status-summary-starting.json`。
- 文中主机一律写 `<服务器地址>`，没有真实地址、口令、令牌。

---

## 1. 通用约定

| 项 | 约定 |
|---|---|
| 传输 | HTTP/1.1，明文（第一版只在局域网用；TLS 留口，见设计第 6 节）。**每个连接一个请求**，应答一律 `Connection: close` |
| 编码 | `application/json; charset=utf-8`；字段名下划线小写；枚举值下划线小写 |
| 版本 | 每个应答顶层带 `schema`（整数，现在是 1）和 `agent_version`（`0.1.0`）。客户端不认识的 `schema` 按「版本对不上」提示 |
| 时间 | ISO 8601 带时区：`2026-11-12T19:20:03+08:00`。日志行的时刻另用 `HH:MM:SS.mmm`（代理收到那一行的本地时间） |
| 没有的值 | `null` 或空串，**不用 0 冒充**（进程不在时 `cpu_pct` 是 `null`、`pid` 是 0） |
| 未知字段 | 客户端一律忽略（serde 缺省就是） |
| 未知取值 | proto 的每个枚举都有 `Unknown` 兜底（`#[serde(other)]`），新代理多出来的取值不会让老面板整份解析失败 |
| 鉴权 | 除 `GET /v1/ping` 外都要 `Authorization: Bearer <令牌>`。不带、带错一律 401，不区分 |
| 客户端标识 | `X-Client: <计算机名>\<用户名>`：只进审计和任务的 `by`，不参与鉴权；代理去掉控制字符、截到 128 个字 |
| `Host` | 代理按它拼快照里的网页地址（第 3.3 节）；没有就用连接的本地地址 |
| 请求体 | 最大 1 MB，必须带 `Content-Length`；**不收分块**（`Transfer-Encoding` → 411）。请求头最大 16 KB（→ 431） |
| 缓存 | 应答都带 `Cache-Control: no-store` |

### 1.1 状态码

| 码 | 什么时候 |
|---|---|
| 200 | 正常 |
| 202 | 收下了（任务、`/v1/refresh`、队列开关没等到确认） |
| 304 | `GET /v1/status?since=<seq>` 等满长轮询、`seq` 还没变（**没有体**） |
| 400 | 请求认不出、参数不对（`bad_request`） |
| 401 | 令牌不对或没带（`unauthorized`），带 `WWW-Authenticate: Bearer realm="deploy-agent"` |
| 404 | 没有这个端点 / 任务（`not_found`）；非 mock 身份下的 `/v1/debug/*` 也是 404 |
| 405 | 端点在、方法不对（`method_not_allowed`），带 `Allow` |
| 409 | 忙（`busy`，带 `busy_job_id`）、危险操作没带 `confirm`（`confirm_required`）、有服务在跑不许改配置（`services_running`）、后端没在跑（`backend_down`）、后端拒绝（`rejected`） |
| 411 / 413 / 431 | 分块请求体 / 体太大 / 头太大 |
| 501 | 这一版做不了（`not_supported`）：单独停一个服务（后端不支持时）、取消任务 |
| 503 | 代理刚起、还没有第一份快照（`not_ready`） |
| 504 | `/v1/fs/probe` 5 秒没探完（掉线的网络盘） |

### 1.2 错误体 `ErrorBody`

所有非 2xx（304 除外）的应答体：

```json
{ "schema": 1, "agent_version": "0.1.0", "error": "busy", "message": "另一个任务（17）在跑，等它结束再操作", "busy_job_id": 17 }
```

`error` 取值见 `deploy_dash_proto::error_code`：`unauthorized`、`bad_request`、`not_found`、`method_not_allowed`、`busy`、`confirm_required`、`services_running`、`backend_down`、`rejected`、`payload_too_large`、`headers_too_large`、`not_supported`、`not_ready`、`internal`。`message` 是给人看的中文，可以直接显示。〔出入〕设计稿 409 的体只有 `{ "busy_job_id" }`，这里统一成 `ErrorBody`。

### 1.3 游标（日志、提示、任务输出共用）

- 行号从 1 起，每个来源各自单调递增。
- 请求 `after=<n>`：要 `n` **之后**的行（`n` 是客户端手里最后一行的行号；从头要传 0）。
- 应答 `next`：这次给到的最后一行的行号；没有新行时等于 `after`。下次原样当 `after` 传回来。
- `reset: true`：客户端先清空这个来源再填。三种情况：`after` 比环里最老的一行还老（离线太久，`dropped` = 中间丢了几行）；`after` 比最新的还新（代理重启过，编号从头来了）；没给 `after`（给最后 `max` 行）。
- `more: true`：环里还有更新的行（受 `max` 限制没给完），马上再要一次。
- `oldest` / `newest`：环里最老、最新一行的行号（空环都是 0）。
- 代理重启后编号从头来；`instance` 变了也说明重启过。

〔出入〕设计稿只写了「`after` 比最老的还老 → `reset`」；`next` 的确切含义、`more`、`oldest`、`newest`、「没给 `after` 给最后几行」是这里定的。

### 1.4 长轮询

- 状态：`GET /v1/status?since=<seq>`，`since` 等于当前 `seq` 时挂着，`seq` 一变马上回（实现：事件进来后攒 30 毫秒再发布，发布后挂着的请求立刻醒；单测要求从事件到应答 < 100 毫秒）；25 秒还没变回 304。`since` 和当前不等（包括比当前大：代理重启过）→ 马上回。
- 日志：`GET /v1/logs?...&wait=<秒>`（0–25，缺省 0 = 不等）。`after` 等于该来源最新行号时挂着，来新行就回；到点回空的 `lines`（200，不是 304）。〔出入〕设计稿说「`after` 等于 `next` 时挂最多 25 秒」，这里改成显式的 `wait`，免得只想看一眼的请求被挂住。
- 同一个客户端对一台代理同时最多需要一个状态长轮询；代理最多同时处理 128 个连接。

---

## 2. 端点一览

| 方法 路径 | 作用 | 应答类型 | 夹具 |
|---|---|---|---|
| `GET /v1/ping` | 探活、版本协商（**不要令牌**） | `Ping` | `ping.json` |
| `GET /v1/info` | 代理和这套部署的静态信息 | `AgentInfo` | `info.json` |
| `GET /v1/status?since=&summary=` | 状态快照（长轮询） | `StatusSnapshot` / `StatusSummary` | `status.json`、`status-summary.json` |
| `GET /v1/logs?source=&after=&max=&wait=` | 日志增量（`/v1/logs/<源>` 同义） | `LogsResponse` | `logs.json` |
| `GET /v1/notices?after=&max=` | 提示（面板的 `UiEvent::Notice`）〔新增〕 | `NoticesResponse` | `notices.json` |
| `POST /v1/jobs` | 下命令 | 202 `JobAccepted` | `job-request.json`（请求）、`job-accepted.json` |
| `GET /v1/jobs?limit=20` | 最近的任务（新的在前，重启后读回） | `JobList` | `jobs.json` |
| `GET /v1/jobs/<id>` | 一个任务 | `JobDetail`（任务字段平铺在顶层） | `job.json` |
| `GET /v1/jobs/<id>/output?after=&max=` | 任务输出增量 | `JobOutput` | `job-output.json` |
| `POST /v1/jobs/<id>/cancel` | 取消 | 这一版一律 501 | — |
| `POST /v1/queue/pause`、`/v1/queue/resume` | 增量同步关 / 开 | `QueueSwitch` | `queue-switch.json` |
| `GET /v1/dbnums` | 库列表 | `DbnumList` | `dbnums.json` |
| `GET /v1/config` | 配置摘要（白名单字段） | `ConfigSummary` | `config.json` |
| `PUT /v1/config/project` | 改 `[project]`，然后自动 `doctor` | 202 `JobAccepted` | `configure-request.json`（请求） |
| `GET /v1/fs/probe?path=` | 探服务器上的一个路径 | `FsProbe` | `fs-probe.json` |
| `POST /v1/refresh` | 马上采一次样（`Command::RefreshNow`）〔新增〕 | 202 `OkResponse` | `refresh.json` |
| `POST /v1/debug/fault` | 注入故障（**只 mock**） | `DebugResponse` | `debug-fault.json`（请求）、`debug-response.json` |
| `POST /v1/debug/fixture` | 切固定场景（**只 mock**）〔新增〕 | `DebugResponse` | `debug-fixture.json`（请求） |

错误体样例：`error-busy.json`、`error-unauthorized.json`。路径常量：`deploy_dash_proto::paths::*`。

没做（第二版）：`GET /v1/events`（SSE）、`GET /v1/fs/dirs`（白名单目录树）、`GET /v1/status/history`。

---

## 3. 状态

### 3.1 `GET /v1/ping`

```json
{ "agent": "deploy-dash-agent", "agent_version": "0.1.0", "schema": 1 }
```

只回这三样，不带套名、不带路径。模拟数据时 `schema` 可以用 `mock_ping_schema` 改掉（冒充老代理，见 6.7）；真实后端一律报真的。

### 3.2 `GET /v1/info`

`AgentInfo`：`agent`、`instance`（这次运行的标识，`<启动时刻>-<进程号>`；变了 = 代理重启过）、`name`、`root`、`stack_version`、`agent_started_at`、`listen`（实际在听的地址）、`caps`、`backend`（「真实服务（stack）」/「模拟数据」，同 `StackDriver::source_label`）、`mock`。

`caps` 现有：`status.long_poll`、`status.summary`、`logs`、`logs.wait`、`notices`、`jobs`、`jobs.output`、`queue`、`dbnums`、`config`、`config.project`、`fs.probe`、`refresh`；mock 身份另有 `debug.fault`、`debug.fixture`。老面板据此知道新代理多了什么。

### 3.3 `GET /v1/status?since=<seq>&summary=0|1`

形状对齐面板的 `StackSnapshot` + `MetricSample` + `BackendJobs`（`model.rs`），样例见 `status.json`。字段：

| 字段 | 说明 | 面板那边 |
|---|---|---|
| `schema`、`agent_version` | | |
| `instance` | 代理这次运行的标识〔新增〕 | 变了就清本地游标 |
| `seq` | 快照每变一次加一 | `since` 用它 |
| `generated_at`、`elapsed_ms` | 后端这一轮采样的时刻、用时 | `StackSnapshot.generated_at / elapsed_ms` |
| `stack` | `{ name, root, version, first_run, config_valid }`；`config_valid` 代理拿不准时是 `null`〔出入：设计写的是布尔〕 | `name`、`root`、`stack_version`、`first_run` |
| `project` | `[project]`；没配置是 `null` | `project` |
| `lock` | `{ held, command, pid, since }` | `lock` |
| `job` | 正在跑的任务摘要（`queued` / `running`），没有是 `null` | `JobStarted` |
| `last_job` | 最近结束的任务摘要〔新增〕 | 看它的 `id` 变化补 `JobFinished`，详情去 `GET /v1/jobs/<id>` |
| `services[]` | **固定四项、固定顺序**：数据库、后端、rs-server、前端 | `services` + `MetricSample.per_svc` |
| `host` | `{ cores, mem_total_gb, commit_free_gb, disks[] }`；还没采到是 `null` | `MetricSample` 其余字段 |
| `backend` | 后端自报的健康；后端不是 `ok` 时是 `null` | `backend` |
| `jobs_summary` | 后端任务；后端不是 `ok` 时是 `null` | `BackendJobs` |
| `guard` | 护栏线，0 = 没配 | `guard` |
| `backend_config_generated`、`backend_exe_note`、`drift[]` | | 同名 |
| `urls` | `{ frontend, backend, ops, rs_server }`（主机名已重写，见下） | `urls`（多一个 `rs_server`） |
| `notice_seq` | `/v1/notices` 最新一条的编号〔新增〕 | 变了就去取 |
| `dbnums_seq` | 库列表每更新一次加一〔新增〕 | 变了就去取 `/v1/dbnums` |
| `log_heads` | 每个日志来源最新的行号，键是来源名〔新增〕；跟着快照发布，最多晚一个采样周期 | 比手里的大就去取 |

`services[]` 每项：`name`（`database` / `backend` / `rs_server` / `frontend`）、`title`、`enabled`、`state`（`ok` / `stopped` / `waiting` / `bad` / `disabled`）、`reason_code`（`""` / `not_running` / `health_failed` / `port_taken` / `unreadable` / `waiting_storage_version`）、`reason`、`checked`、`next`、`port`、`pid`（进程不在是 0）、`ours`、`started_at`（可为 `null`）、`version`、`log`、`cpu_pct`、`mem_gb`（进程不在或没采到是 `null`）。

`host.disks[]`：`{ drive, role, free_gb, total_gb }`，`role` 是 `root`（运行目录所在盘）、`system`（系统盘）；同一个盘只给一项、`role` 写 `root,system`。面板的 `MetricSample.disk` / `sys_disk` 按 `role` 取（`root,system` 两边都填它）。

`jobs_summary`：`{ paused, running, queued, held, rows[], failures_24h[], upgrade }`。`rows[]`：`{ dbnum, state（running / queued）, intent, phase, since, task_id }`；`failures_24h[]`：`{ at, dbnum, message }`；`upgrade`：`{ done, total }` 或 `null`〔出入：设计稿把 `upgrade` 放在 `backend` 里，这里跟 `model::BackendJobs` 放〕。`backend.requests_total` 后端现在没有，是 `null`（模拟数据有）。

**`summary=1`**（`StatusSummary`，样例 `status-summary.json`、`status-summary-starting.json`）：只回 `schema`、`agent_version`、`instance`、`seq`、`generated_at`、`stack.name`、`stack.first_run`、`stack.lock_command`、`job`、`services[]` 的 `name / state / reason_code`、`host.commit_free_gb`、`host.disks[]` 的 `drive / role / free_gb`。给左侧服务器列表和顶部汇总用。

- `stack.first_run`：同完整快照的 `stack.first_run`——这套还没配置过（`stack.toml` 不在，或 `[project]` 没填工程名）。面板显示「未配置」。
- `stack.lock_command`：占着 `run\stack.lock` 的那条 stack 命令（`up`、`down`、`init`……）；没占锁是空串，占着锁但读不出命令名是 `unknown`。同完整快照的 `lock.held` / `lock.command`（`LockInfo::held_command`）。没有代理任务时（例如有人在服务器上直接跑了 `stack up`）后台那几台也能据此显示「启动中」「停止中」。
- 这两个字段 22:0x 才加（只加不改，`schema` 不升）：老代理不发，客户端读成 `false` / 空串（`#[serde(default)]`）；老面板读新代理的摘要，多出来的字段忽略。

**`urls` 按请求重写**：快照里的回环地址（`127.x`、`localhost`、`[::1]`、`0.0.0.0`）换成客户端连上来用的主机名（`Host` 头去掉端口）；代理配置 `agent.toml` 的 `public_url` 配了就用它的主机名，不看 `Host`；已经不是回环地址的（例如 stack.toml 里 rs-server 配的 `public_url`）原样不动。端口和路径不动。

**还没有快照**（代理刚起）：不带 `since` 时最多等 3 秒，仍没有回 503 `not_ready`。

---

## 4. 日志与提示

### 4.1 `GET /v1/logs?source=<源>&after=<行号>&max=<n>&wait=<秒>`

- 来源（`LogSourceName`）：`database`、`backend`、`rs_server`、`frontend`（四个服务的日志文件尾）、`job`（stack 命令的输出和代理里业务流程的说明，面板 `LogSource::Job`）、`stack`（`<根>\logs\stack\` 最新文件的尾巴，代理自己读）、`agent`（代理自己的日志，面板可当 `LogSource::Panel` 显示）。〔出入〕设计的写法是 `/v1/logs/<源>`，这里以查询参数为准，路径写法也认。
- `max` 缺省 800、上限 5000；`wait` 见 1.4。每个来源的环 5,000 行。
- 应答 `LogsResponse`：`source`、`from`（这次第一行的行号，没有行是 0）、`next`、`reset`、`dropped`、`more`、`oldest`、`newest`、`file`（正在读的文件；`job` / `agent` 是空串）、`lines[]`、`instance`。
- 一行 `LogLine`：`{ n, t, level, text }`。`level`（`info` / `warn` / `error`）是代理按关键字猜的（同面板 `events::guess_level`），提示里照旧写明是猜的。
- **隐去在代理端做**：一行里含 `password`、`passwd`、`pwd=`、`secret`、`token`、`authorization`、`bearer `、`v_password`、`口令`、`密码`（不分大小写；`agent.toml` 的 `redact_keywords` 可改）就整行换成「（这一行可能含账号信息，已隐去）」再进环。〔出入〕设计说照 `stack.ps1` 的 `log_redact_pattern`（正则）；这一版用关键字表，没接那条正则。任务输出文件里也是隐去后的。

### 4.2 `GET /v1/notices?after=<n>&max=<n>`〔新增〕

后端发的提示（面板 `UiEvent::Notice`：「另一个任务在跑」「生成模型完成」「增量同步没关回去」……）。游标同 1.3，`max` 缺省 50，环里留最近 200 条。一条 `Notice`：`{ n, at, kind（info / success / warning / error）, title, message }`。快照里的 `notice_seq` 变了就来取。

任务被后端拒绝（`rejected`）时，原因同时出现在任务的 `message` 和这里的一条提示里——客户端只显示一次（照 stack.rs：被拒的任务只弹提示，不发开始 / 结束）。

---

## 5. 任务

### 5.1 `POST /v1/jobs`

请求体 `JobRequest`（样例 `job-request.json`）：

```json
{ "kind": "init", "args": { "dbnums": [7997, 8000] }, "confirm": true, "note": "" }
```

| `kind` | `args` | 后端命令（`BackendCommand` ↔ 面板 `Command`） | 要 `confirm: true` |
|---|---|---|---|
| `up` | `{ "no_init": true }`（可省，代理只会 `up -NoInit`） | `Up` | 否 |
| `down` | `{ "force": false }` | `Down { force }` | **是**（含 `force`） |
| `start` | `{ "service": "rs_server" }` | `StartService` | 否 |
| `stop` | `{ "service": …, "force": false }` | `StopService` | **是** |
| `restart` | `{ "service": … }` | `RestartService` | 服务在跑时**是** |
| `init` | `{ "dbnums": [u32…] }`（1–200 个正整数） | `Init` | **是** |
| `doctor` | `{}` | `Doctor` | 否 |
| `configure` | `{ "project": { … } }` | `Configure`，成功后接着 `Doctor` | **是** |
| `preview_update` | `{ "dbnums": […] }` | `PreviewUpdate`〔新增〕 | 否 |
| `incremental_update` | `{ "dbnums": […] }` | `IncrementalUpdate`〔新增〕 | 否 |

`args` 没用到的字段不写（序列化时省掉）。`note` 原样进审计（截到 500 字）。

危险操作名单按设计第 10 节第 9 条：`down`、`down -Force`、`stop`、运行中的 `restart`、`init`、`configure`；`up`、`start`、`doctor`、队列开关只记审计。**没带 `confirm: true` 回 409 `confirm_required`**〔出入：设计 4.4 写的是 400，按派工改成 409〕，也进审计（`result = confirm_required`）。

代理先在自己这里判断，判过了才交给后端（`BackendCommand`），顺序：

1. 参数不对 → 400 `bad_request`（例如 `init` 的库号列表是空的、`start` 没给 `service`、`kind` 不认识、`project` 里有引号 / 控制字符、`dir` 不是一段名字、`code` 不是整数）。
2. 危险操作没带 `confirm` → 409 `confirm_required`。
3. `configure` 时有服务在跑（任一 `pid` 不是 0）→ 409 `services_running`。
4. `stop`、运行中的 `restart`，而后端不能单独停一个服务（`AgentBackend::can_stop_single_service()` 为 false，真实的 stack 0.4.0 就是）→ 501 `not_supported`。
5. 真实后端、`init` 时后端没在跑 → 409 `backend_down`。
6. 同一时刻只跑一个任务：已有 `queued` / `running` 的 → 409 `busy`，带 `busy_job_id`。
7. 收下：代理编号（重启后接着编）、写结果文件、交给后端、记审计 → **202 `JobAccepted`** `{ job_id, job }`。

### 5.2 任务的状态

| `state` | 意思 |
|---|---|
| `queued` | 代理收下了、交给后端了，后端还没开始（后端可能正忙着采样） |
| `running` | 后端发了 `JobStarted` |
| `done` | 后端发了 `JobFinished`：看 `exit_code`（`down` 后端在忙时是 6、什么都没停，原因在 `busy_reasons[]`；`doctor` 有问题时是 4，报告在 `result.doctor`） |
| `rejected` | 后端没接：没开始就发了非「成功」的提示（例如「另一个任务在跑」「不能单独重启」），原因在 `message`，`exit_code` 是 `null` |
| `cancelled` | 预留（这一版没有取消） |
| `lost` | 代理重启过、不知道结果；或者收下 120 秒后端还没开始 |

〔出入〕设计只有 `running` / `done` / `cancelled` / `lost`；`queued`、`rejected` 是代理把「HTTP 当场回答」和后端的事件流对上时要的。

`configure` 是两步：写 `[project]` 成功（退出码 0）后自动跑 `doctor`，整个算一个任务：`exit_code` 是 doctor 的，`message` 是「配置已保存；<doctor 的话>」，报告在 `result.doctor`。〔出入〕设计说「回自动跑的 doctor 的 `job_id`」，这里是同一个任务号。

起停类任务（`up`、`down`、`start`、`stop`、`restart`）在跑时带 `progress { done, total, step }`：从服务状态推出来（不解析文字输出）——`total` 是要动的服务数，`done` 是已经到目标状态的，`step` 是正在等的那个服务。

### 5.3 任务对象 `JobInfo`

`id`、`kind`、`args`、`state`、`label`（界面上的叫法，同 `model::JobKind::label`）、`exit_code`、`message`、`busy_reasons[]`、`queued_at`、`started_at`、`finished_at`、`by`（`X-Client`；代理自己起的是 `agent`；没带头是「（没带 X-Client）」）、`from`（客户端 IP）、`confirm`、`note`、`output_lines`、`output_file`、`progress`、`result`（结束了才有：`{ exit_code, message, doctor, preview }`）。

- `GET /v1/jobs/<id>`：`JobDetail` = `schema` + `agent_version` + 任务字段平铺（同设计 4.4 的样例）。代理内存里留最近 50 个，更老的 404。
- `GET /v1/jobs?limit=20`：`JobList { jobs[] }`，新的在前，`limit` 1–50。
- `GET /v1/jobs/<id>/output?after=&max=`：`JobOutput { job_id, state, from, next, reset, more, lines[] }`，游标同 1.3（`after` 缺省 0 = 从头）。读回的老任务第一次被问到时从文件里读。
- `POST /v1/jobs/<id>/cancel`：这一版一律 501（后端接口里没有取消；`up` 按设计本来就不许取消），也进审计。

落盘：`<根>\run\gui\jobs\<编号>.log`（一行一条：`HH:MM:SS.mmm<Tab>级别<Tab>正文`）和 `<编号>.result.json`（就是 `JobInfo`，收下时写一份、每次状态变化重写，先写 `.tmp` 再改名）。代理重启后读回最近 50 个，还是 `queued` / `running` 的标成 `lost`。〔出入〕设计写的文件名是 `<编号>-<命令>.out.log`，按派工用 `<编号>.log`。

### 5.4 面板这边怎么对上 `UiEvent`

| `UiEvent` | 从哪来 |
|---|---|
| `Snapshot` / `Metrics` / `BackendJobs` | `/v1/status`（`services[]` + `host` + `jobs_summary`） |
| `SnapshotFailed` | 客户端自己（连不上、超时） |
| `JobStarted` | 快照的 `job` 出现新 `id`，或 `POST` 的应答 / 跟进看到 `running` |
| `JobFinished` | 快照的 `last_job.id` 变了 → `GET /v1/jobs/<id>`；`rejected` 的不发开始 / 结束、只弹提示 |
| `LogLines` | `/v1/logs` |
| `Notice` | `/v1/notices` |
| `Doctor` | 任务的 `result.doctor` |
| `DownBusy` | `exit_code = 6` 的 `down` / `stop` 的 `busy_reasons` |
| `Dbnums` | `/v1/dbnums` |
| `UpdatePreview` | `preview_update` 任务的 `result.preview` |
| `HistorySeed` | `/v1/debug/*` 应答的 `history_seed`（只 mock） |

| `Command` | 请求 |
|---|---|
| `Up` / `Down` / `StartService` / `StopService` / `RestartService` / `Init` / `Doctor` / `PreviewUpdate` / `IncrementalUpdate` | `POST /v1/jobs` |
| `Configure` | `PUT /v1/config/project`（或 `POST /v1/jobs` 的 `configure`） |
| `ListDbnums` | `GET /v1/dbnums` |
| `SetIncrements(on)` | `POST /v1/queue/resume`（开）/ `pause`（关） |
| `RefreshNow` | `POST /v1/refresh` |
| `InjectFault` / `Fixture` | `POST /v1/debug/fault` / `fixture`（只 mock） |

---

## 6. 其余端点

### 6.1 `POST /v1/queue/pause`、`POST /v1/queue/resume`

增量同步关 / 开（后端队列暂停 / 恢复出队）。请求体可空。代理先判：后端没在跑 → 409 `backend_down`；正在生成模型（`init` 任务在跑）→ 409 `busy`。然后交给后端，等快照里的 `backend.queue_paused` 翻过来：等到了回 200 `{ paused, pending: false }`；后端发了警告 / 错误提示 → 409 `rejected`（`message` 是它的话）；20 秒还没等到 → 202 `{ paused: null, pending: true }`，结果看之后的快照。都进审计。〔出入〕设计回 `{ "paused": bool }`，这里多了 `pending`。

### 6.2 `GET /v1/dbnums`

`DbnumList { at, seq, dbnums[] }`，一行 `{ dbnum, initialized, db_type, name, file_size, pieces }`（`pieces` 数不出来是 `null`）。一分钟内拿过就直接给（顺手让后端再要一次）；从没拿到过就让后端去要、最多等 10 秒（真实后端没在跑时不等，回空列表、`at: null`）。数据库口令只在代理里用。

### 6.3 `GET /v1/config`、`PUT /v1/config/project`

`GET /v1/config` → `ConfigSummary`：`project`、`project_defaults`（现在总是 `null`）、`services[] { name, enabled, port }`、`listen { loopback_only }`、`guard { min_commit_free_gb, abort_commit_free_gb, max_backend_gb, min_disk_free_gb, protect_ports[] }`、`init { dbnums[], auto_new }`、`backend_config { generated, drift[] }`、`fs { browse_roots[] }`。有 `stack.toml` 就读它的白名单字段（`[backend.env]`、`[rs_server.env]`、`config_set` 一概不读不发）；没有（mock、还没配置）就从快照里凑。

`PUT /v1/config/project` 的请求体就是 `project` 那个对象，**外加 `confirm`**（和可选的 `note`），样例 `configure-request.json`：

```json
{ "e3d_root": "D:/<工程根>", "name": "<项目名>", "dir": "", "code": "1516", "mdb": "ALL", "e3d_version": "e3d31", "confirm": true, "note": "" }
```

走 5.1 的同一条路（任务 `configure`），回 202 `JobAccepted`。

### 6.4 `GET /v1/fs/probe?path=<路径>`

`FsProbe { path, exists, is_dir, readable }`。**只探这三样，不列目录、不回文件名和大小**〔出入：设计稿回 `dir_count` 和最多 5 个子目录名 `sample`，按派工去掉〕。`readable`：目录以读权限打开句柄（要的是「列目录」的权限，但不真去列），文件以读打开。`path` 要是完整路径；不收 UNC（`\\服务器\共享`）和设备路径（`\\?\`、`\\.\`）——探 UNC 会让代理去连别的机器、还可能交出账户凭据；有控制字符、超过 1024 字 → 400。5 秒没探完 → 504。

### 6.5 `POST /v1/refresh`〔新增〕

让后端马上采一次样（`Command::RefreshNow`），回 202 `{ ok: true }`。结果看下一份快照。

### 6.6 `POST /v1/debug/fault`、`POST /v1/debug/fixture`（只 mock）

只在 mock 身份（`AgentBackend::is_mock()`）下开，否则 404。请求体 `{ "fault": "rs_server_exit" | "field_incident" }`、`{ "fixture": "unconfigured" | "stopped" | "running_healthy" }`；不认识的取值 400。代理交给后端、等它推一轮快照（最多 1 秒），回 `DebugResponse { ok, history_seed }`：切场景和「现场故障」带 15 分钟历史（`HistorySeed { step_secs: 5, states[4][], cpu[4][], mem[4][], disk[], commit[], queue[] }`，每个服务一组、按 数据库 / 后端 / rs-server / 前端；`cpu` / `mem` 里 `null` = 当时没有进程；`states[i][k] = { ago_secs, state }`），别的是 `null`。都进审计。〔`/v1/debug/fixture` 是新增的：面板 `Command::Fixture` 只有 mock 认，自检截图要它〕

### 6.7 `/v1/ping` 冒充别的协议版本（只 mock）〔新增〕

给面板自检「版本对不上」那几句提示用：代理配置 `mock_ping_schema = <n>` 时，`GET /v1/ping` 的 `schema` 报 `n`（例如 0 = 老代理，2 = 比面板新），别的端点照旧报 1。只在 mock 身份（`AgentBackend::is_mock()`）下有效；真实后端配了也不理，`agent.log` 记一句「mock_ping_schema 只在模拟数据时有效」。开法（起代理之前设好，运行中不能切）：

- `<根>\run\gui\agent.toml` 里写一行 `mock_ping_schema = 0`：`deploy-dash.exe --agent --mock` 和 `deploy-agent-dev --mock` 都认；
- 或者 `deploy-agent-dev --mock … --ping-schema 0`（盖过 `agent.toml`）。

起来时 `agent.log` 记一句「模拟：/v1/ping 报协议 0（…）」。缺省文件不写这一项。

---

## 7. 安全（服务端）

- **监听**：`<根>\run\gui\agent.toml` 的 `listen`，**缺省 `127.0.0.1:8600`**；远程要显式改成 `0.0.0.0:8600` 或某块网卡的地址（命令行 `--listen` 优先）。端口不许在保护名单里（本机已知别人的端口 + stack.toml `[guard] protect_ports`）。按安装根取名的单实例互斥量 `Local\AiosDeployDashAgent-<根路径 SHA-256 前 16 位>`，第二个实例退出码 2。
- **防火墙、登录自启**（安装类子命令，不过网）：`open-firewall`、`install-autostart`、`uninstall-autostart` **缺省只打印**要执行的命令（`--dry-run` 同义），加 `--apply` 才执行。`open-firewall` 的规则只放入站 TCP 本端口、只限代理的 exe、只在 Domain / Private，对方地址缺省本网段 `remoteip=localsubnet`；`--remote-ip <网段>` 可以换成具体的地址、网段、范围，`any`、`/0`、比 IPv4 /8（IPv6 /7）还宽的都不收——没有「不限地址」。
- **令牌**：第一次起来时生成（32 字节系统随机数，base64url 不带 `=`，43 个字），写 `<根>\run\gui\agent.token`：先建空文件、`icacls /inheritance:r /grant:r *<当前账户 SID>:(F)`，再写内容、改名就位〔出入：设计写 `:R`；这里给当前账户 `(F)`，换令牌要改写文件；别的账户一样没有任何权限〕。内存里只留 SHA-256，比较是常量时间（`subtle`）。令牌文件换了（`rotate-token`）代理自己重读，旧令牌立刻 401。令牌不进日志、不进审计、`Debug` 打成 `••••`。
- **401**：每次延迟 300 毫秒；同一来源一分钟超过 20 次就延迟 2 秒。记审计（刷屏时每 20 次记一次）。
- **审计**：`<根>\logs\gui\agent-audit.jsonl`，一行一个 JSON `{ t, from_ip, client, action, args, job_id, confirm, result }`；满 10 MB 改名 `.1`（盖掉旧的 `.1`）。`action`：`job.<kind>`（含被 400 / 409 / 501 挡下的，`result` 写原因）、`job.cancel`、`queue.pause` / `queue.resume`、`debug.fault` / `debug.fixture`、`auth.401`。`args` 是请求里的 `args`（没有口令）。
- **能做的事的上限**：只会把白名单命令交给后端（第 5 节那张表）、读白名单文件、探路径（不列目录）、转发队列开关。没有「执行任意命令」「读任意文件」「写任意配置」的接口。
- 代理自己的日志：`<根>\logs\gui\agent.log`（满 10 MB 改名 `.1`），也进 `agent` 日志来源。

---

## 8. 代理侧接口（不过网，给面板的适配器看）

`deploy_agent::AgentBackend` 和面板的 `StackDriver` 一一对应：`start(sink: EventSink)` ↔ `start(events)`，`send(BackendCommand)` ↔ `send(Command)`，`shutdown()` ↔ `shutdown(keep_services)`（服务一律留着），`label()` ↔ `source_label()`，`is_mock()` ↔ `!is_real()`；另有带缺省实现的 `can_stop_single_service()`（缺省 true；包真实驱动的适配器应返回 false）。`BackendEvent` 与 `UiEvent` 同名同序，`BackendCommand` 与 `Command` 同名同序，`deploy_agent::JobKind` 对 `model::JobKind`。入口：`run_agent(AgentConfig, Box<dyn AgentBackend>)`（阻塞，已有实例时返回 `AlreadyRunning`），`start_agent(…)`（异步，返回 `AgentHandle`，单测和嵌入用）。

---

## 9. 和设计第 4 节的出入（以本文为准）

| # | 设计第 4 节 | 本文 | 为什么 |
|---|---|---|---|
| 1 | `GET /v1/logs/<源>?after=` | `GET /v1/logs?source=<源>&after=`（路径写法也认） | 派工单的写法；两种都收 |
| 2 | 日志「`after` 等于 `next` 时挂最多 25 秒」 | 显式 `wait=<秒>`，缺省不等；到点回 200 空行 | 只想看一眼的请求不被挂住 |
| 3 | `after` / `next` 只举了例子 | 1.3 定死：`after` = 手里最后一行，`next` = 这次最后一行；加 `more`、`oldest`、`newest`；不给 `after` 给最后 `max` 行 | 客户端要能写对 |
| 4 | 危险操作没带 `confirm` 回 400 | 409 `confirm_required` | 派工单 |
| 5 | `/v1/fs/probe` 回 `exists`、`is_dir`、`dir_count`、`sample` | 只回 `exists`、`is_dir`、`readable` | 派工单：不列目录 |
| 6 | 409 体 `{ "busy_job_id" }`；没有统一错误体 | 所有错误都是 `ErrorBody { error, message, busy_job_id }` | 客户端一套解析 |
| 7 | 任务状态 `running` / `done` / `cancelled` / `lost` | 多 `queued`、`rejected` | 代理先收下、再看后端接不接 |
| 8 | `PUT /v1/config/project` 回自动 `doctor` 的 `job_id` | 同一个任务号，`configure` 两步做完才结束，报告在 `result.doctor`；请求体多 `confirm` | 一个确认框对一个任务 |
| 9 | 任务输出 `run\gui\jobs\<编号>-<命令>.out.log` | `<编号>.log` + `<编号>.result.json` | 派工单 |
| 10 | `POST /v1/jobs/<id>/cancel` 能取消（`up` 除外） | 一律 501 | 后端接口（`StackDriver`）里没有取消 |
| 11 | `stop` / `restart` 等丙路 `-Service`，没有前回 501 | 后端声明不能单独停时 501；模拟后端能做 | 由 `can_stop_single_service()` 决定 |
| 12 | 没有 | 新增 `/v1/notices`、快照 `notice_seq` | `UiEvent::Notice` 要有去处 |
| 13 | 没有 | 新增 `/v1/refresh` | `Command::RefreshNow` |
| 14 | 只有 `/v1/debug/fault` | 新增 `/v1/debug/fixture`；两者应答带 `history_seed` | `Command::Fixture`、`UiEvent::HistorySeed`（自检要可重现） |
| 15 | 没有 | 快照新增 `instance`、`last_job`、`dbnums_seq`、`log_heads`；`job` 带 `label`、`service`、`progress` | 客户端少拉、能补 `JobFinished`、能看出代理重启 |
| 16 | `backend.upgrade` | `jobs_summary.upgrade { done, total }` | 跟 `model::BackendJobs` |
| 17 | `stack.config_valid` 布尔 | 可为 `null` | 代理（驱动）拿不准时不冒充 |
| 18 | `POST /v1/queue/*` 回 `{ paused }` | `QueueSwitch { paused, pending }`；后端拒绝 409 `rejected` | 后端慢时不假装知道结果 |
| 19 | `urls` 三项 | 多 `rs_server`；只重写回环地址 | 远程「打开网页」要它 |
| 20 | 隐去照 `log_redact_pattern` 正则 | 关键字表（可配） | 这一版没接那条正则 |
| 21 | `kind` 八种 | 多 `preview_update`、`incremental_update` | 对上 `Command::PreviewUpdate` / `IncrementalUpdate` |
| 22 | 令牌文件 ACL `:R` | 当前账户 `(F)`，别人无权限 | 代理要能换令牌 |
| 23 | `/v1/events`（SSE）、`/v1/fs/dirs` | 没做 | 设计本来就放第二版 |
| 24 | `summary=1` 只有套名、服务、磁盘 | 摘要的 `stack` 多 `first_run`、`lock_command` | 后台那几台分得出「未配置」「启动中」（fleet-ui 20:50 的请求） |
| 25 | 没有 | mock 的 `/v1/ping` 能报别的 `schema`（6.7） | 自检要测得到「老代理」那句 |
