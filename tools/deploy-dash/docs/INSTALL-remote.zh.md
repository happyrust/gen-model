# 部署面板 · 远程监控安装说明（草稿）

- 草稿：agent-server，2026-11-12 21:5x（计划待定 7、集成步 J5）；22:0x 按续派改了防火墙、登录自启两段（安装类子命令改成缺省只打印、`--apply` 才执行，防火墙规则自带本网段）。依据的是当时的代码，不是设想：代理 `spike\deploy-agent`、协议 `spike\deploy-dash-proto\PROTOCOL.md`、子命令 `deploy_agent::install`（打印的原样是用 `deploy-agent-dev.exe` 实际打出来的）、面板一侧的提示文字（`data\remote.rs`、`fleet_state.rs`、`ui\fleet.rs`）。
- 版本：代理 0.1.0，协议 `schema = 1`。面板和代理要同一版（整包覆盖）。
- 文中的地址、令牌都是占位：服务器 `192.0.2.10`、本网段 `192.0.2.0/24`、令牌 `<令牌>`、离线包解开的目录 `<根>`（`stack.ps1`、`stack.toml` 所在的目录）、面板 exe 所在目录 `<面板目录>`。

---

## 0. 先看：现在能用到哪一步

这份说明写的是**集成做完以后**的用法。写这份草稿时（21:5x）几块还没合进面板；J1 在 21:52 合进去了（agent-client 记的），文中标「待 J1」的那些现在都在代码里，J4 实测后再核一遍。标「待 J2」的还没有。

| 部分 | 现在 | 说明 |
|---|---|---|
| 代理本体（HTTP 服务、令牌、审计、任务表、日志） | 做完了，70 个单测过 | 库在 `spike\deploy-agent` |
| `deploy-dash.exe --agent …`（代理身份和它的子命令）、本机模式、`--direct` | J1 已合入 | 子命令和开发用的 `deploy-agent-dev.exe` 同一套（附录 A），安装类那三个的开关见下面第二行 |
| 面板连别的电脑 | 底层已合入（J1） | 界面要等 J2 |
| 服务器列表、服务器抽屉、「测试连接」（界面） | **待 J2** | 代码写好了（`ui\fleet.rs`、`fleet_state.rs`），还没接进面板。第 3 节的提示文字照代码抄 |
| 防火墙规则、登录自启 | **缺省只打印，`--apply` 才真建** | `--apply` 那条路还没在任何机器上跑过，留到验收（J6）用户在场；平时把打印的命令人工执行。见 2.4、2.5 |
| `deploy-dash.exe --agent` 的这三个子命令跟上新规矩 | **待 agent-client 改 `main.rs`** | 现在的 `main.rs` 还是旧写法：不带 `--dry-run` 就直接执行，不认 `--apply`、`--remote-ip`。在它换过来之前，用 `deploy-dash.exe` 时一律带上 `--dry-run`（新旧两种写法都只打印）；`deploy-agent-dev.exe` 已经是新规矩 |
| TLS（加密）、按人分权 | **第二版** | 第一版没有，见 1.2 |

---

## 1. 它是什么、不是什么

### 1.1 它是什么

- 服务器上多跑一个**没有窗口的「stack 代理」**：就是同一个 `deploy-dash.exe`，换成 `--agent` 身份（待 J1）。它在服务器本机看四个服务的状态、进程、磁盘内存、日志，代跑 `stack.ps1`（一键启动、全部停止、生成模型、自检、改配置），然后在一个端口上用 HTTP + JSON 提供给面板。
- 别的电脑上的 `deploy-dash.exe`（面板）在「服务器列表」里加上这台服务器的地址、端口和令牌，就能看监控、下命令，和坐在服务器前一样。
- 服务器上自己打开面板（本机模式）时，面板也是经 127.0.0.1 上的这个代理；代理没在跑，面板会自己把它隐藏拉起（待 J1）。关面板时代理**留着**，四个服务也照常跑。
- 代理不起停服务以外的东西，也**不替四个服务加鉴权**：数据库、后端、rs-server、前端各自监听什么还是 `stack.toml` 说了算。面板对后端的查询、开关都经代理走，所以服务器可以把 `[listen] loopback_only` 收紧成 `true`，远程面板照样全功能（网页除外，见 4.6）。
- 代理只会做白名单里的事：交给 `stack.ps1` 的固定几条命令、开关增量同步（转给后端的队列接口）、读状态和日志文件、探一下某个路径在不在（只回「在不在、是不是目录、读不读得了」，不列目录）。没有「执行任意命令」「读任意文件」「写任意配置」的接口。数据库口令、`stack.toml` 原文、环境变量从不发出去；日志里含口令字样（password、token、口令、密码 等）的行整行隐去再发。

### 1.2 它不是什么（第一版的限制）

- **只在局域网里直连**：面板直接连服务器的 IP 和端口。不经公网、不做反向代理、不做端口映射。
- **一台服务器一个共享令牌**：所有面板用同一个令牌，没有用户名、口令、角色。审计里记的「谁」（计算机名\用户名）是面板自报的，不是验证过的身份。
- **没有 TLS（不加密）**。这意味着：
  - 令牌、状态、日志、命令都以**明文**在网线上走。同一网段里能抓包的人（或者被攻破的电脑、做了端口镜像的交换机）能看到令牌。
  - 拿到令牌的人就能以面板的身份下命令：起停服务、生成模型、改 `[project]`。界面上的确认框和协议里的 `confirm` 只防手滑、不防坏人；代理会把每条命令和来源 IP 记进审计。
  - 所以：只在可信的局域网里开；防火墙只放行本网段（2.4）、不放 Public 网络；怀疑令牌泄露就马上换（2.3）；不需要远程时把监听改回 127.0.0.1（5.1）。
  - TLS（`https://` + 首次连接记住证书指纹）、按人分权、Windows 服务形态都放在**第二版**，现在都没有。
- **不是 Windows 服务**：代理是「当前账户登录时」起的后台进程（任务计划，2.5）。那个账户没登录，代理就不在；面板会显示「连不上」，四个服务不受影响。

---

## 2. 服务器端：装代理

### 2.1 前提

- 离线包已经解开在 `<根>`，面板 `deploy-dash.exe` 已经能在这台服务器上以真实模式打开（exe 旁边有 `deploy-dash.real.toml`，或者用 `--real --stack-root <根>`），四个服务能一键启动。
- **用平时起停服务的那个 Windows 账户来装**，并且**不要**在「以管理员身份运行」的窗口里装登录自启。原因：`stack.ps1` 认进程看「进程号 + 启动时刻 + 程序路径」，起和停要同一个账户、同一个权限级别；代理也一样（设计 3.3）。
- 防火墙那一步要**管理员权限**（另开一个「以管理员身份运行」的窗口，只做那一条）。
- 端口：缺省 **8600**。先看它空着（没有输出就是空的）：

```powershell
Get-NetTCPConnection -LocalPort 8600 -State Listen -ErrorAction SilentlyContinue
```

- 这台服务器连的网络要是「域」或「专用」类别（防火墙规则不放 Public），看一下：

```powershell
Get-NetConnectionProfile | Select-Object InterfaceAlias, NetworkCategory
```

### 2.2 让代理听局域网：改 `agent.toml`

代理的配置在 `<根>\run\gui\agent.toml`。第一次起代理、或者第一次运行任何 `--agent` 子命令时，会自动写一份带注释的缺省文件。缺省**只听本机**：

```toml
listen = "127.0.0.1:8600"
public_url = ""
browse_roots = []
```

要让别的电脑连，把 `listen` 改成 `"0.0.0.0:8600"`，存盘，重启代理（结束进程再起，见 2.7）。

- **端口、地址都改在 `agent.toml` 里，不要只用命令行的 `--listen`**：服务器上的本机面板是读 `agent.toml` 去找代理端口的（待 J1），两处不一致它会去连错的端口。
- 想只听某一块网卡也可以写那块网卡的地址（例如 `"192.0.2.10:8600"`），但这样服务器上的本机面板连 127.0.0.1 就连不上，会提示「listen 只听一块网卡…改成 0.0.0.0 或 127.0.0.1」。一般用 `0.0.0.0`。
- `public_url`：经过端口映射、或者给服务器起了好记的名字时填（例如 `"http://<服务器名>"`）。填了，远程面板上「打开网页」的地址就用它的主机名；不填就用面板连上来时用的地址。
- 代理不许听在「保护名单」上的端口（本项目已知的服务端口，如 8009、8022、9098、3100 等，加上 `stack.toml` 里 `[guard] protect_ports`），会报「端口 … 是这台机器上别的服务的（在保护名单里），代理不能听它」并退出。
- 改成非本机地址后，代理每次起来会在自己的日志里记一句「在听非本机地址 …：局域网里的机器拿到令牌就能连」——这是提醒，不是错误。

### 2.3 令牌

- **在哪**：`<根>\run\gui\agent.token`，一行 43 个字符（字母、数字、`-`、`_`）。第一次起代理、或第一次运行 `show-token` 时生成（系统随机数）。
- **权限**：去掉继承，只给**当前账户**完全控制，别的账户（包括 Administrators 组）不在列表里。所以 `show-token`、`rotate-token` 要用**跑代理的同一个账户**运行，不然它读不到、代理也读不到。核对：

```powershell
icacls "<根>\run\gui\agent.token"
```

  应当只有一行，是这个账户（可能显示成 `S-1-5-…` 的形式）带 `(F)`。

- **看令牌**（只在服务器本机看，抄给要连的面板）：

```powershell
deploy-dash.exe --agent show-token --root "<根>"
```

  控制台打出一行令牌。不要贴进聊天工具、邮件、工单；当面抄或者用 U 盘带。

- **换令牌**（怀疑泄露、有人离开、定期换）：

```powershell
deploy-dash.exe --agent rotate-token --root "<根>"
```

  打出「已换新令牌（旧的立刻失效，在跑的代理会自己重读）」和新令牌。**不用重启代理**：它发现令牌文件变了就用新的，拿旧令牌的面板马上收到「拒绝了令牌」，到「编辑服务器…」换成新的即可。服务器上开着的本机面板是打开时读一次令牌，换完也会报一次，关掉重开就好（待 J1，以实际为准）。

### 2.4 防火墙：人工执行（管理员窗口）

**`open-firewall` 缺省只打印**：把要执行的 `netsh` 命令拼好、打出来，什么都不建（`--dry-run` 是同义词）。加 `--apply` 才真建（会弹 UAC）——这条路写了，但**还没在任何机器上跑过**，留到验收演示（J6）用户在场时用；平时把打印出来的那一行拿到「以管理员身份运行」的窗口里人工执行。

先把 2.2 的端口定下来（它打印的端口取自 `agent.toml`），再：

```powershell
deploy-dash.exe --agent open-firewall --root "<根>" --dry-run
```

（`--dry-run` 本来可以不写；但 agent-client 把 `main.rs` 换成新规矩之前，`deploy-dash.exe` 不带它会直接执行，所以先照写。）

它打印的原样是（路径换成了占位）：

```
（只打印，没执行）要在「以管理员身份运行」的窗口里跑的是：
netsh advfirewall firewall add rule name="AIOS 部署代理" dir=in action=allow protocol=TCP localport=8600 program="<面板目录>\deploy-dash.exe" profile=domain,private remoteip=localsubnet enable=yes

对方地址只放本网段（localsubnet）；要换网段用 --remote-ip <网段>。
要真执行加 --apply（会弹 UAC）。
```

（`deploy-dash.exe` 换过来之前，第一行还是旧的「（--dry-run，不执行）要在管理员窗口里跑的是：」，后两句没有；`netsh` 那一行一样，已经带 `remoteip=localsubnet`。）

这条规则只放行：入站、TCP、本端口、只限 `deploy-dash.exe` 这个程序、只在「域」「专用」网络（不放 Public）、**对方地址只限本网段**（`remoteip=localsubnet`：和这台服务器的网卡在同一网段的电脑）。四个服务的端口不因为它多开任何东西。

**换网段**：看面板的电脑和服务器不在同一网段（中间隔着路由器）时，用 `--remote-ip` 写那些电脑所在的网段，打印出来的 `remoteip=` 就换成它：

```powershell
deploy-dash.exe --agent open-firewall --root "<根>" --remote-ip 192.0.2.0/24
```

可以写一个网段（`192.0.2.0/24`）、一个地址（`192.0.2.50`）、一段范围（`192.0.2.50-192.0.2.80`），多个用逗号隔开，也可以和 `localsubnet` 并列。**没有「不限地址」的写法**：`any`、`*`、`0.0.0.0/0`，以及比 /8 还宽的网段或范围，它都拒绝并说明原因。（`deploy-dash.exe` 认 `--remote-ip` 要等 agent-client 换过来；在那之前用 `deploy-agent-dev.exe open-firewall … --remote-ip …` 打印，或者把打印出来的 `remoteip=` 那一项照这个规矩手改。）

核对规则：

```
netsh advfirewall firewall show rule name="AIOS 部署代理" verbose
```

注意：规则绑的是 `deploy-dash.exe` 的完整路径。面板换了目录，要删掉旧规则（5.2）再按新路径重建。

### 2.5 登录自启：人工执行（普通窗口、同一账户）

`install-autostart` 也是**缺省只打印**：打出一段 PowerShell，不建任务计划；加 `--apply` 才真建（同样还没在任何机器上跑过，留到 J6）。在**普通（不提权）**的 Windows PowerShell 窗口里，用**跑服务的那个账户**（`--dry-run` 照写，理由同 2.4）：

```powershell
deploy-dash.exe --agent install-autostart --root "<根>" --dry-run
```

它打印的原样是（路径换成了占位）：

```
（只打印，没执行）要在普通（不提权）的 Windows PowerShell 窗口里、用平时起停服务的那个账户跑的是：
$ErrorActionPreference = 'Stop'
$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$a = New-ScheduledTaskAction -Execute '<面板目录>\deploy-dash.exe' -Argument '--agent --root <根>' -WorkingDirectory '<根>'
$t = New-ScheduledTaskTrigger -AtLogOn -User $user
$p = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -StartWhenAvailable
Register-ScheduledTask -TaskName 'AIOS 部署代理' -Action $a -Trigger $t -Principal $p -Settings $s -Description '部署面板的 stack 代理：当前账户登录时在后台起（不提权）' -Force | Out-Null
Write-Output "已建任务计划：$user 登录时起代理"

要真执行加 --apply。
```

（`<根>` 里有空格时，它会自动给路径加上双引号。`deploy-dash.exe` 换过来之前，第一行是旧的「（--dry-run，不执行）要跑的 PowerShell：」，最后一句没有，中间的脚本一样。）

意思是：任务名「AIOS 部署代理」；**当前账户登录时**起 `deploy-dash.exe --agent --root <根>`；**不提权**（`-RunLevel Limited`）；不限运行时长；已经在跑就不再起第二个。

- 把中间那段（从 `$ErrorActionPreference` 到 `Write-Output` 那一行）贴进同一个普通窗口执行。建好后不用注销，马上起一次：

```powershell
Start-ScheduledTask -TaskName 'AIOS 部署代理'
```

- 看它的状态：`Get-ScheduledTask -TaskName 'AIOS 部署代理' | Get-ScheduledTaskInfo`。`LastTaskResult` 是 267009（0x41301）表示正在跑；是 2 表示同一个根上已经有一个代理（2.7）；是 1 表示起不来（看 4.3、4.6 的日志）。
- 打印时如果带了 `--listen …`，任务的参数里就会写死这个地址。按 2.2 的说法，监听地址写在 `agent.toml` 里就够了，这里**不要**带 `--listen`。
- 「拒绝访问」：有的机器策略不许普通账户建「登录时」触发的任务。这条路没有实测过；遇到了先记下报错原文，别改用管理员窗口建（那样代理的权限级别就和起服务的人不一样了，见 2.1）。

### 2.6 确认代理在听

在服务器上：

```powershell
Get-NetTCPConnection -LocalPort 8600 -State Listen | Select-Object LocalAddress, LocalPort, OwningProcess
Invoke-RestMethod http://127.0.0.1:8600/v1/ping
```

- `LocalAddress` 是 `0.0.0.0`：别的电脑能连。是 `127.0.0.1`：只听本机（2.2 没改，或者改了没重启代理）。
- `/v1/ping` 不要令牌，应答固定是这三样：

```json
{ "agent": "deploy-dash-agent", "agent_version": "0.1.0", "schema": 1 }
```

在要看面板的那台电脑上：

```powershell
Test-NetConnection 192.0.2.10 -Port 8600        # TcpTestSucceeded : True 才说明网络和防火墙通了
Invoke-RestMethod http://192.0.2.10:8600/v1/ping
```

要连令牌一起验，用面板的「测试连接」（第 3 节）；非用命令行不可：

```powershell
Invoke-RestMethod http://192.0.2.10:8600/v1/info -Headers @{ Authorization = 'Bearer <令牌>' }
```

（命令行里的令牌会留在历史记录里，测完关掉这个窗口。）`/v1/info` 回 `name`（套名）、`stack_version`、`listen`、`mock: false` 就对了。

代理自己的日志：`<根>\logs\gui\agent.log`。起来时第一行是「代理 0.1.0 起来了：听 0.0.0.0:8600，安装根 …，后端「真实服务（stack）」」。

### 2.7 退出码 2：这个根上已经有一个代理

代理按**安装根**只许跑一个（互斥量 `Local\AiosDeployDashAgent-<根路径的哈希>`）。第二个会打印「这个安装根已经有一个代理在跑了（单实例互斥量被占着）」并以退出码 2 退出。**这不是故障**，常见的来源：登录自启已经起了一个；服务器上的本机面板已经把它拉起来了（待 J1）；有人用 `--direct` 开着面板（这时面板占着代理的互斥量，代理起不来；待 J1）。

处理：

1. 先 ping（2.6）：通的话代理已经在了，什么都不用做。
2. 要让新配置（`agent.toml` 的 `listen`）生效，就结束在跑的那个再起：

```powershell
Get-CimInstance Win32_Process -Filter "Name = 'deploy-dash.exe'" | Where-Object CommandLine -like '*--agent*' | Select-Object ProcessId, CommandLine
Stop-Process -Id <上面那个进程号>
Start-ScheduledTask -TaskName 'AIOS 部署代理'
```

结束代理**不会**停四个服务：服务由 `stack.ps1` 起、不挂在代理名下。正在跑的 `stack.ps1` 命令会不会被带着中止，还没实测（计划待定 5，J4 测）——所以别在「一键启动」「生成模型」跑到一半时结束代理。

### 2.8 代理在服务器上留下的东西

| 路径 | 是什么 |
|---|---|
| `<根>\run\gui\agent.toml` | 代理配置（2.2） |
| `<根>\run\gui\agent.token` | 令牌（2.3），只有当前账户能读 |
| `<根>\run\gui\jobs\<编号>.log`、`<编号>.result.json` | 经代理下的每个任务的输出和结果，代理重启后读回最近 50 个 |
| `<根>\run\gui\jobs\stack\` | `stack.ps1` 每次命令的原样输出（待 J1） |
| `<根>\logs\gui\agent.log`（满 10 MB 改名 `.1`） | 代理自己的日志 |
| `<根>\logs\gui\agent-worker.log` | 代理里采样器、任务执行的日志（待 J1） |
| `<根>\logs\gui\agent-audit.jsonl`（满 10 MB 改名 `.1`） | 审计：每条命令一行 `{ t, from_ip, client, action, args, job_id, confirm, result }`，包括被拒的和令牌不对的（`auth.401`）。令牌不进审计 |

### 2.9 装完核对

- [ ] `agent.toml` 的 `listen` 是 `0.0.0.0:8600`（或你定的端口），代理重启过
- [ ] `icacls` 看令牌文件只有一个账户
- [ ] 防火墙规则在，`profile` 是 Domain、Private，`remoteip` 是本网段
- [ ] 任务计划在，`LastTaskResult` 是 267009
- [ ] 服务器上 `Invoke-RestMethod http://127.0.0.1:8600/v1/ping` 有应答
- [ ] 另一台电脑上 `Test-NetConnection` 通、面板「测试连接」显示代理版本

---

## 3. 另一台电脑：在面板里加一台服务器

（界面待 J2；连非本机地址待 J1。下面的字段和提示照现有代码写。）

### 3.1 填什么

打开「服务器…」抽屉（顶部汇总条右边的「服务器…」，或左侧列表底下的「添加服务器…」），点「添加服务器」：

| 栏 | 填什么 |
|---|---|
| 名称（必填） | 自己认得的名字，不能和已有的重名，也不能叫「本机」 |
| 地址（必填） | 服务器的 IP 或主机名，例如 `192.0.2.10`。可以连端口一起写成 `192.0.2.10:8600`；写了 `http://` 会自动去掉；不要带路径、用户名、口令。**第一版只有 http**，不要写 `https://` |
| 端口 | 空着就是 8600；和地址里写的端口要一致，不一致会提示 |
| 令牌（必填） | 服务器上 `show-token` 打出来的那一串（2.3）。输入时显示成圆点；保存后**再也不显示**，编辑时留空就是不改，重新输入就是替换 |
| 备注 | 可空 |

先点「测试连接」，通了再「保存」。

服务器自己那台面板上，「本机」一项是自动列出的（exe 旁有 `stack.ps1` 或 `deploy-dash.real.toml`），不用填令牌，也删不掉。

### 3.2 「测试连接」的几种结果

它先问 `/v1/ping`（不要令牌），再带令牌问 `/v1/info`，所以几种失败分得开：

| 看到的话 | 意思 | 怎么办 |
|---|---|---|
| 「代理 0.1.0 · plant-stack · stack 0.4.0 · 往返 9 ms」（绿勾） | 连上了，令牌对 | 保存 |
| 「代理在，但不认这个令牌。在服务器上运行 deploy-dash --agent show-token，把令牌重新抄一遍。」 | 端口上是代理，但 401：令牌不对或没填（代理不区分这两种） | 重新抄令牌；服务器刚换过令牌的话用新的 |
| 「连不上 192.0.2.10:8600：<原因>。核对地址和端口，确认服务器上的代理在运行、防火墙放行了这个端口。」 | 网络层面没到代理：地址或端口错、代理没在跑、代理只听本机、防火墙没放行、服务器网络是 Public…… | 按 4.2 排查。原因里写「端口 8600 上的程序不像部署代理」时，是那个端口上跑着别的程序，端口填错了 |
| 「192.0.2.10:8600 上的代理是 0.0.9（协议 0），本面板要协议 1：把服务器上的部署包整包覆盖成新版。」 | 老代理：它的协议版本比本面板旧 | 服务器上换成和本面板同一版的部署包 |
| 「192.0.2.10:8600 上的代理比本面板新（…，协议 2）：把这台电脑上的面板换成新版。」 | 代理比面板新，协议对不上 | 这台电脑上的面板换成新版 |

### 3.3 连上以后

- 左侧列表里每台一行：状态图标、名字、`4/4` 这类计数或「连不上」。选中那台，主区域就是它的一屏面板，和在服务器前看到的一样。
- 选中的那台连不上时，顶上一条横幅「连不上 <服务器名>：<原因>。…」，旧数据变暗留着，自动重试；也可以点「重试」「编辑服务器…」（服务器上的「本机」还多一个「重新拉起代理」）。
- 令牌不对时横幅是「<服务器名> 拒绝了令牌：在服务器上运行 deploy-dash --agent show-token 抄下令牌，再点「编辑服务器…」重新输入。」连着 3 次不对就不再自动重试，改好令牌后点「重试」。
- 版本对不上时提示「代理版本对不上」，写清代理和面板各要哪个协议版本。

### 3.4 这台电脑上存了什么

`%LOCALAPPDATA%\AiosDeployDash\` 下：`servers.toml`（服务器列表）和 `panel.toml`（面板设置，例如列表折没折、上次选中哪台）。令牌是用 Windows DPAPI **按当前用户**加密后存的（`dpapi:…`），文件里没有明文；把这个文件拷到别的电脑或别的用户下解不开，那几台会标成「要重新输入」令牌。

---

## 4. 常见故障

### 4.1 401：「拒绝了令牌」

- 令牌抄错（43 个字符，注意 `-`、`_`、大小写），或者服务器上换过令牌（2.3）。在服务器上 `show-token` 再抄一遍，到「编辑服务器…」里重新输入。
- 代理那边：每个 401 应答故意慢 0.3 秒；同一个地址一分钟里超过 20 次，之后每次慢 2 秒。所以令牌错的面板会显得「卡」，这是有意的。
- 代理把 401 记进审计（`action` 是 `auth.401`，带来源 IP），`agent.log` 里第 1 次和每满 20 次记一句「… 的请求令牌不对或没带（一分钟内第 N 次）」。看到陌生 IP 在试令牌，换令牌并查是谁。

### 4.2 连不上

按这个顺序查：

1. **代理在不在**：服务器上 `Invoke-RestMethod http://127.0.0.1:8600/v1/ping`。不通就看任务计划的 `LastTaskResult`（2.5）和 4.6 的日志。
2. **只听本机**：`Get-NetTCPConnection -LocalPort 8600 -State Listen` 的 `LocalAddress` 是 `127.0.0.1` → 改 `agent.toml`（2.2）后重启代理（2.7）。
3. **防火墙**：从客户端 `Test-NetConnection 192.0.2.10 -Port 8600`。`TcpTestSucceeded` 是 False 而服务器上 ping 是通的 → 规则没建、端口不对、`program=` 路径不是正在跑的那个 exe、或者客户端不在 `remoteip` 允许的网段里（2.4）。
4. **网络是 Public**：服务器上 `Get-NetConnectionProfile` 显示 `Public` 时，这条只放 Domain、Private 的规则不起作用。让管理员把这块网卡的网络类别改成专用（`Set-NetConnectionProfile -InterfaceAlias <网卡> -NetworkCategory Private`，要管理员），**不要**给规则加 Public。
5. **地址、端口填错**：提示里有「不像部署代理」就是端口上是别的程序。
6. 地址写了 `https://`：第一版没有 TLS，改成不带前缀的 IP。

### 4.3 端口被占、端口不许用

- **被别的程序占了**：代理起不来，以退出码 1 退出，原因是「听不了 0.0.0.0:8600：…（os error 10048）」。手工在命令行起时打在控制台；登录自启时看 `agent-worker.log`（待 J1），任务计划的 `LastTaskResult` 是 1。看是谁占的：

```powershell
Get-NetTCPConnection -LocalPort 8600 -State Listen | ForEach-Object { Get-Process -Id $_.OwningProcess }
```

  换一个空端口：改 `agent.toml` 的 `listen`，重新打印并执行防火墙那条命令（端口变了，规则要跟着变；旧规则删掉，见 5.2），客户端的端口栏也改成新的。
- **在保护名单上**：报「端口 … 是这台机器上别的服务的（在保护名单里），代理不能听它；换一个，缺省是 8600」，退出码 1。换端口，同上。
- 服务器上的本机面板说「127.0.0.1:8600 上是别的程序（…），不是部署代理」：同一回事，换端口（待 J1）。

### 4.4 两台电脑的时间不一样

**不影响**连接、令牌和「连不上」的判断（这些都不看时间）。只影响**显示**：

- 顶栏「更新于 15:32:08」、任务的开始结束时刻，显示的是**服务器**的时间；
- 「运行了多久」「排队等了多久」是用**这台电脑**的现在减去服务器给的时刻算的：服务器快了，这些时长会显示「—」；服务器慢了，时长会偏长。

两台都对时就没事（在域里会自动对时；工作组里让管理员开 Windows 时间服务）。看一眼：`w32tm /query /status`。

### 4.5 从 PowerShell 7（pwsh）里起东西：PowerShell 5.1 的 `PSModulePath` 坑

现象：某个 `stack.ps1` 命令或 PowerShell 脚本失败，报「The 'Get-Acl' command was found in the module 'Microsoft.PowerShell.Security', but the module could not be loaded」这一类（命令名、模块名会不一样）。

原因：在 **pwsh 7** 窗口里起了一个程序（例如 `deploy-dash.exe`、`cmd.exe`），这个程序**再去起** Windows PowerShell 5.1（`powershell.exe`）时，5.1 继承了 pwsh 7 的 `PSModulePath`，去加载 7 版的模块，加载失败。pwsh 7 **直接**起 `powershell.exe` 时它会自己把这个变量修好，所以在 pwsh 7 里直接敲 `powershell.exe …` 没事；隔了一层就出事（本机实测过这两种情况）。

现在的代码里已经避开了：面板 / 代理起 `stack.ps1` 时、代理的 install 子命令起 PowerShell 时，都先去掉了这个变量。所以正常使用碰不到。如果还是碰上（例如老版本的面板，或者自己写的脚本隔了一层），这样绕：

- 最省事：别在 pwsh 7 里起，开一个 `cmd` 窗口，或开始菜单里的「Windows PowerShell」（5.1）窗口来起；登录自启走任务计划，不经过 pwsh 7，不受影响。
- 非在 pwsh 7 里不可时，起之前清掉、起完放回：

```powershell
$old = $env:PSModulePath; $env:PSModulePath = $null
try { & "<面板目录>\deploy-dash.exe" --agent --root "<根>" } finally { $env:PSModulePath = $old }
```

- 在 `cmd` 里：`set PSModulePath=` 之后再起。

### 4.6 其它

- **远程面板上「打开网页」打不开**：代理只转状态和命令，不转网页。网页地址是 `http://<服务器地址>:<前端端口>/` 这类，服务器的 `stack.toml` 里 `[listen] loopback_only = true` 时，这几个网页只能在服务器本机打开；要远程看网页，按 stack 自己的说明改 `loopback_only` 并另外放行那几个端口——本说明不涉及，也不建议为了面板去开。
- **代理起不来，看哪里**：手工起时看控制台；登录自启时看 `<根>\logs\gui\agent-worker.log`（待 J1）和 `agent.log`、任务计划的 `LastTaskResult`。`agent.toml` 写坏了会报「<路径> 写得不对：…」或「listen = "…" 不是「地址:端口」」，退出码 1——这一句只打在控制台、不进日志文件（读配置在开日志之前），登录自启时只看得到 `LastTaskResult` 是 1；在命令行里手工起一次就能看到原因。
- **令牌文件读不了**：报「读不了令牌文件 …」。多半是用另一个账户运行过 `show-token`，令牌文件归了那个账户。用跑代理的账户删掉 `agent.token`（删不掉就请管理员删），再起代理，会生成新的；所有面板重新抄令牌。
- **任务一直是「排队」**：真实服务时，代理把命令交给采样器，采样器正在采样时（等各服务的健康接口、后端的库列表，一轮常见十几秒，最坏几十秒）任务会在「排队」停一会儿；120 秒还没开始会标成「丢了」（`lost`）。

---

## 5. 卸载

### 5.1 只是不让别的电脑连了（服务器上的本机面板照用）

1. 删防火墙规则（管理员窗口）：

```
netsh advfirewall firewall delete rule name="AIOS 部署代理"
```

2. `agent.toml` 的 `listen` 改回 `"127.0.0.1:8600"`，重启代理（2.7）。
3. 换一次令牌（2.3），让发出去的旧令牌作废。

### 5.2 这台服务器不再用代理

1. 删登录自启（普通窗口、同一账户）。打印命令：`deploy-dash.exe --agent uninstall-autostart --root "<根>" --dry-run`（`--dry-run` 照写，理由同 2.4），原样是：

```
（只打印，没执行）要在普通的 Windows PowerShell 窗口里跑的是：
$ErrorActionPreference = 'Stop'
Unregister-ScheduledTask -TaskName 'AIOS 部署代理' -Confirm:$false
Write-Output '已删掉登录自启'

要真执行加 --apply。
```

   把中间三行贴进窗口执行。

2. 结束代理进程（2.7 那两行）。四个服务不受影响。
3. 删防火墙规则（5.1 第 1 步）。
4. 删文件：`<根>\run\gui\agent.toml`、`agent.token`、`<根>\run\gui\jobs\`、`<根>\logs\gui\agent.log*`、`agent-worker.log*`。审计 `agent-audit.jsonl*` 按需要先留档再删。
5. 注意：J1 之后，服务器上的面板以真实模式打开时，会自己在 127.0.0.1 上拉起代理，并重新生成 `agent.toml`、`agent.token`——那是本机模式本身的一部分，只听本机。真要在这台服务器上完全不用代理，只能用面板的 `--direct`（直接管 stack 的逃生口，计划第二版删掉），或者不在这台服务器上开面板。

### 5.3 另一台电脑

- 在「服务器…」抽屉里点那台的「删除…」：「只是不再显示它，服务器上什么都不会变。」
- 整个清掉：关面板，删 `%LOCALAPPDATA%\AiosDeployDash\`（服务器列表、加密的令牌、面板设置都在里面）。

---

## 附录 A：子命令一览

正式入口是 `deploy-dash.exe --agent …`（J1 已合入）。开发用的 `deploy-agent-dev.exe` 有同样的子命令和参数（不带 `--agent`；它只能起模拟数据的代理，`deploy-agent-dev.exe --help` 列出全部开关）。

| 命令 | 做什么 | 要不要提权 |
|---|---|---|
| `--agent --root <根> [--listen 地址:端口]` | 起代理，阻塞到进程被结束。退出码 0 正常、1 出错、2 这个根上已有一个 | 不要 |
| `--agent show-token --root <根>` | 打印令牌（没有就生成一个） | 不要（同一账户） |
| `--agent rotate-token --root <根>` | 换新令牌并打印；在跑的代理自己重读 | 不要（同一账户） |
| `--agent open-firewall --root <根> [--remote-ip <网段>] [--apply]` | 打印 `netsh` 建规则的命令（对方地址缺省本网段）；`--apply` 才建 | 打印不要；执行那条命令（或 `--apply`）要 |
| `--agent install-autostart --root <根> [--apply]` | 打印建任务计划的 PowerShell；`--apply` 才建 | 不要，也**不要**提权 |
| `--agent uninstall-autostart --root <根> [--apply]` | 打印删任务计划的 PowerShell；`--apply` 才删 | 不要 |

三个安装类子命令**缺省只打印**（`--dry-run` 是同义词，和 `--apply` 一起给会报错），加 `--apply` 才执行（`open-firewall` 会弹 UAC）。`--apply` 这条路这一版**还没有在任何机器上跑过**，留到验收（J6）用户在场；平时把打印的命令人工执行。现在的 `deploy-dash.exe --agent` 还是旧写法（不带 `--dry-run` 就执行、不认 `--apply` / `--remote-ip`），等 agent-client 换过来；在那之前一律带 `--dry-run`。`open-firewall` 打印的规则绑的是当前这个 exe 的完整路径（`program=`）；从别的目录、别的 exe 运行它，打印出来的路径就不对。

## 附录 B：协议的几处要紧约定（给排查的人）

完整的在 `spike\deploy-dash-proto\PROTOCOL.md`。

- 除 `GET /v1/ping` 外都要 `Authorization: Bearer <令牌>`；不带、带错一律 401，不区分。
- 每个连接一个请求、`Connection: close`；请求头最大 16 KB、请求体最大 1 MB。
- 危险操作（全部停止、停一个服务、运行中的重启、生成模型、改配置）请求里要带 `"confirm": true`，否则 409；面板的确认框就是做这个的。
- 代理最多同时处理 128 个连接；状态是长轮询，每台面板对每台服务器同一时刻只挂一个请求。
