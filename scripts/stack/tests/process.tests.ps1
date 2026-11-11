# 进程管理：停止只停该停的、认领只认该认的。
# 这一组用假的进程表——把入口里「查进程、查端口、结束进程、等一会儿、打印」这几样换成假的，一个真进程都不碰。
# （在真进程上做的那一条在 process-live.tests.ps1 里。）

$script:FakeProcs = @{}
$script:FakeListen = @{}
$script:FakeKilled = New-Object System.Collections.ArrayList
$script:FakeSaid = New-Object System.Collections.ArrayList
$script:FakeOnSleep = $null
$script:FakeT0 = [datetime]'2026-01-01T08:00:00'

function Reset-FakeWorld {
    $script:FakeProcs = @{}
    $script:FakeListen = @{}
    $script:FakeKilled.Clear()
    $script:FakeSaid.Clear()
    $script:FakeOnSleep = $null
    $script:FakeCtrlMode = 'exit'
    $script:FakeQuitMode = 'exit'
    if ($null -ne $script:FakeCtrlCalls) { $script:FakeCtrlCalls.Clear() }
    if ($null -ne $script:FakeCaptureCalls) { $script:FakeCaptureCalls.Clear() }
    $script:FakeBusy = @{ Known = $true; Busy = $false; Reasons = @() }
    $script:FakeBusyAsked = 0
}

# 往假的进程表里加一个进程。$At 是它的创建时间（相对一个固定起点的秒数）；$Path 为空 = 读不到程序路径。
function Add-FakeProc([int]$Id, [int]$Parent, [string]$Path, [double]$At, [int[]]$Listen = @()) {
    $name = ''
    if ($Path) { $name = Split-Path -Leaf $Path } else { $name = 'unknown.exe' }
    $script:FakeProcs[$Id] = [pscustomobject]@{ ProcessId = $Id; ParentProcessId = $Parent; Name = $name; ExecutablePath = $Path; CommandLine = $Path; CreationDate = $script:FakeT0.AddSeconds($At) }
    foreach ($port in $Listen) {
        if (-not $script:FakeListen.ContainsKey($port)) { $script:FakeListen[$port] = @() }
        $script:FakeListen[$port] += $Id
    }
}

# ── 换掉入口里碰系统的函数（只在这个文件的作用域里有效）──
function Get-CimInstance {
    [CmdletBinding()]
    param([Parameter(Position = 0)][string]$ClassName, [string]$Filter = '')
    if ($ClassName -eq 'Win32_OperatingSystem') { return [pscustomobject]@{ FreeVirtualMemory = 64 * 1024 * 1024 } }
    if ($Filter -match '^ProcessId=(\d+)$') { $want = [int]$Matches[1]; if ($script:FakeProcs.ContainsKey($want)) { return $script:FakeProcs[$want] }; return $null }
    if ($Filter -match '^ParentProcessId=(\d+)$') { $want = [int]$Matches[1]; return @($script:FakeProcs.Values | Where-Object { $_.ParentProcessId -eq $want }) }
    if ($Filter -match "^Name='(.+)'$") { $want = $Matches[1]; return @($script:FakeProcs.Values | Where-Object { $_.Name -ieq $want }) }
    return @($script:FakeProcs.Values)
}
function Get-ListenTable([switch]$Fresh) { return $script:FakeListen }
function Stop-Process {
    [CmdletBinding()]
    param([int]$Id, [switch]$Force)
    [void]$script:FakeKilled.Add($Id)
    $script:FakeProcs.Remove($Id)
    foreach ($port in @($script:FakeListen.Keys)) { $script:FakeListen[$port] = @($script:FakeListen[$port] | Where-Object { $_ -ne $Id }) }
}
# 假的钟：等一会儿 = 把钟往前拨，不真等。这样「最多等 15 秒」这类循环一下子就过去。
$script:FakeNow = [datetime]'2026-01-02T09:00:00'
function Get-Date { return $script:FakeNow }
function Start-Sleep {
    param([double]$Milliseconds = 0, [double]$Seconds = 0)
    $script:FakeNow = $script:FakeNow.AddMilliseconds($Milliseconds + $Seconds * 1000)
    if ($null -ne $script:FakeOnSleep) { & $script:FakeOnSleep }
}
function Say([string]$Text = '', [string]$Color = '') { [void]$script:FakeSaid.Add($Text) }

# 「请它自己退出」的两条路也换成假的，记下每次是怎么调的：
#   发 Ctrl+C 的小进程：$script:FakeCtrlMode = 'exit'（发出去了，那个进程和包着它的 cmd 随即退出）/
#                       'hang'（发出去了，它就是不退）/ 'refuse'（它的控制台上还有别人，没发）
#   nginx -s quit：     $script:FakeQuitMode = 'exit'（主进程、工作进程、外面那层 cmd 都退了）/ 'fail'（这条命令自己报错）
$script:FakeCtrlMode = 'exit'
$script:FakeQuitMode = 'exit'
$script:FakeCtrlCalls = New-Object System.Collections.ArrayList
$script:FakeCaptureCalls = New-Object System.Collections.ArrayList
function Remove-FakeProc([int]$Id) {
    $script:FakeProcs.Remove($Id)
    foreach ($port in @($script:FakeListen.Keys)) { $script:FakeListen[$port] = @($script:FakeListen[$port] | Where-Object { $_ -ne $Id }) }
}
function Send-ConsoleCtrl($Cfg, [int]$ProcessId, [string]$Ticks, [int[]]$Allowed) {
    [void]$script:FakeCtrlCalls.Add(@{ Pid = $ProcessId; Ticks = $Ticks; Allowed = @($Allowed) })
    if ($script:FakeCtrlMode -eq 'refuse') { return @{ Sent = $false; Why = '它的控制台上还有别的进程（进程号 999），发了会连带打断它们' } }
    if ($script:FakeCtrlMode -eq 'exit') {
        $parent = 0
        if ($script:FakeProcs.ContainsKey($ProcessId)) { $parent = [int]$script:FakeProcs[$ProcessId].ParentProcessId }
        Remove-FakeProc $ProcessId
        if ($parent -gt 1 -and $script:FakeProcs.ContainsKey($parent) -and $script:FakeProcs[$parent].Name -ieq 'cmd.exe') { Remove-FakeProc $parent }
    }
    return @{ Sent = $true; Why = '' }
}
function Invoke-Capture($Cfg, [string]$Exe, [string]$ArgLine, [string]$Cwd, [int]$TimeoutSec = 20) {
    [void]$script:FakeCaptureCalls.Add(@{ Exe = $Exe; ArgLine = $ArgLine; Cwd = $Cwd })
    if ($ArgLine -notmatch '-s quit$') { return @{ Ok = $false; Text = '（假的：这里不真跑程序）'; TimedOut = $false; ExitCode = 1 } }
    if ($script:FakeQuitMode -eq 'fail') { return @{ Ok = $false; Text = 'nginx: [error] OpenEvent("Global\ngx_quit_100") failed'; TimedOut = $false; ExitCode = 1 } }
    foreach ($p in @($script:FakeProcs.Values | Where-Object { $_.ExecutablePath -ieq $Exe })) {
        $parent = [int]$p.ParentProcessId
        Remove-FakeProc ([int]$p.ProcessId)
        if ($parent -gt 1 -and $script:FakeProcs.ContainsKey($parent) -and $script:FakeProcs[$parent].Name -ieq 'cmd.exe') { Remove-FakeProc $parent }
    }
    return @{ Ok = $true; Text = ''; TimedOut = $false; ExitCode = 0 }
}
# 后端忙不忙：缺省「问得到、闲着」。要它忙就改 $script:FakeBusy；$script:FakeBusyAsked 数问了几次。
$script:FakeBusy = @{ Known = $true; Busy = $false; Reasons = @() }
$script:FakeBusyAsked = 0
function Get-BackendBusy($Cfg) { $script:FakeBusyAsked += 1; return $script:FakeBusy }

# ── 一份假的安装目录的配置（只解析，不要求文件存在）──
$root = New-TestDir 'proc'
Write-TestFile (Join-Path $root 'stack.toml') @'
[project]
e3d_root = "e3d"
name = "Sample"
code = 1516
e3d_version = "e3d31"

[database]
exe = "bin/surreal.exe"
port = 51009

[backend]
exe = "bin/aios-database.exe"
port = 51022
config_template = "config/DbOption.factory.toml"
resource_dir = "resource"
schema_root = "schema"

[rs_server]
exe = "bin/rs-server.exe"
port = 51098
template_dir = "rs-server"

[frontend]
kind = "nginx"
exe = "bin/nginx/nginx.exe"
root = "www"
port = 51100

[guard]
protect_ports = [8009]
'@
$cfg = Get-StackConfig (Join-Path $root 'stack.toml')
$exeDb = $cfg.Database.Exe
$exeBackend = $cfg.Backend.Exe
$exeNginx = $cfg.Frontend.Exe
$exeCmd = 'C:\Windows\System32\cmd.exe'

# 照入口的格式给一个假进程造一条状态记录。
function New-FakeRecord([int]$Id, [int]$Port, [int]$WrapperId = 0) {
    $p = Get-ProcInfo $Id
    $wrapperTicks = ''
    if ($WrapperId -gt 0) { $wrapperTicks = Get-ProcTicks (Get-ProcInfo $WrapperId) }
    return @{ pid = $Id; startTicks = (Get-ProcTicks $p); startedAt = (Format-ProcClock $p); exe = [string]$p.ExecutablePath; marker = ''; port = $Port; wrapperPid = $WrapperId; wrapperTicks = $wrapperTicks; log = 'old.log'; adopted = '' }
}
function Get-Said { return ($script:FakeSaid -join "`n") }

Test 'Select-TrueChildren 只认创建时间不早于父进程的' {
    $parentTicks = $script:FakeT0.AddSeconds(10).ToUniversalTime().Ticks
    $candidates = @(
        [pscustomobject]@{ ProcessId = 200; ParentProcessId = 100; CreationDate = $script:FakeT0.AddSeconds(5) }    # 比「父亲」还早：撞号撞来的
        [pscustomobject]@{ ProcessId = 201; ParentProcessId = 100; CreationDate = $script:FakeT0.AddSeconds(11) }   # 真孩子
        [pscustomobject]@{ ProcessId = 202; ParentProcessId = 999; CreationDate = $script:FakeT0.AddSeconds(12) }   # 别人的孩子
        [pscustomobject]@{ ProcessId = 203; ParentProcessId = 100; CreationDate = $null }                            # 创建时间读不到
        [pscustomobject]@{ ProcessId = 204; ParentProcessId = 100; CreationDate = $script:FakeT0.AddSeconds(10) }   # 同一时刻：算
        [pscustomobject]@{ ProcessId = 100; ParentProcessId = 100; CreationDate = $script:FakeT0.AddSeconds(10) }   # 自己
    )
    $kids = Select-TrueChildren $candidates 100 $parentTicks
    Assert-Equal @(201, 204) @($kids | ForEach-Object { $_.ProcessId }) '只剩真孩子'
    Assert-Equal 0 @(Select-TrueChildren $candidates 100 0).Count '父进程的创建时间不知道：一个都不认'
    Assert-Equal 0 @(Select-TrueChildren $null 100 $parentTicks).Count '没有候选'
}

Test 'Get-ChildProcs：父进程不在了、或者进程号换了主人，一个都不认' {
    Reset-FakeWorld
    Add-FakeProc 100 1 $exeBackend 10
    Add-FakeProc 201 100 $exeBackend 11
    Add-FakeProc 200 100 'C:\Windows\explorer.exe' 1
    $ticks = Get-ProcTicks (Get-ProcInfo 100)
    Assert-Equal @(201) @(Get-ChildProcs 100 $ticks | ForEach-Object { $_.ProcessId }) '记录的启动时间对得上：只认真孩子'
    Assert-Equal @(201) @(Get-ChildProcs 100 | ForEach-Object { $_.ProcessId }) '不给启动时间：按现在这个进程的创建时间算'
    $wrong = [string]($script:FakeT0.AddSeconds(500).ToUniversalTime().Ticks)
    Assert-Equal 0 @(Get-ChildProcs 100 $wrong).Count '进程号换了主人：一个都不认'
    $script:FakeProcs.Remove(100)
    Assert-Equal 0 @(Get-ChildProcs 100 $ticks).Count '父进程不在了：一个都不认'
}

Test 'down：撞号撞来的「假孩子」不动，真的子进程和外面那层 cmd 连带收掉' {
    Reset-FakeWorld
    Add-FakeProc 90 1 $exeCmd 99                                  # 外面包的那层 cmd
    Add-FakeProc 100 90 $exeNginx 100 @(51100)                    # nginx 主进程（本套记着的）
    Add-FakeProc 101 100 $exeNginx 101                            # nginx 工作进程：真孩子
    Add-FakeProc 200 100 'C:\Windows\explorer.exe' 1              # 父进程号也是 100，但比 100 早创建得多：它的父亲是很久以前的另一个 100
    Add-FakeProc 201 100 'D:\other\important.exe' 50              # 同上，另一个不相干的
    Save-StackState $cfg @{ frontend = (New-FakeRecord 100 51100 90) }
    $code = Invoke-Down $cfg $false
    Assert-Equal 0 $code 'down 的退出码'
    Assert-Equal @(90, 100, 101) @($script:FakeKilled | Sort-Object) '只停了主进程、它的真孩子、外面那层 cmd'
    Assert-True ($script:FakeProcs.ContainsKey(200) -and $script:FakeProcs.ContainsKey(201)) '撞号的两个进程都还在'
    Assert-Equal 0 (Read-StackState $cfg).Count '记录清掉了'
}

Test 'down：占着受保护端口的子进程不动' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Add-FakeProc 401 400 'D:\prod\surreal.exe' 60 @(8009)        # 真是它的孩子，但占着受保护的端口
    Add-FakeProc 402 400 'D:\helper\helper.exe' 61               # 真孩子，没占受保护端口
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    [void](Invoke-Down $cfg $false)
    Assert-Equal @(400, 402) @($script:FakeKilled | Sort-Object) '停了后端和普通的孩子'
    Assert-True ($script:FakeProcs.ContainsKey(401)) '占着受保护端口的那个还在'
    Assert-Match (Get-Said) '受保护的端口 8009' '说了为什么没动它'
}

Test 'down：记录里的进程自己占着受保护端口——不认它是本套的，不停' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022, 8009)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    [void](Invoke-Down $cfg $false)
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-Match (Get-Said) '不停任何进程' '说了不停'
}

Test 'down：记录里的进程号现在是另一个进程（启动时间对不上）——不停' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    $record = New-FakeRecord 400 51022
    $script:FakeProcs.Remove(400); $script:FakeListen = @{}
    Add-FakeProc 400 1 'C:\Windows\explorer.exe' 900            # 同一个进程号，后来分给了别的进程
    Save-StackState $cfg @{ backend = $record }
    [void](Invoke-Down $cfg $false)
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-True ($script:FakeProcs.ContainsKey(400)) '那个进程还在'
}

Test 'down -WhatIf：只列名单，不停、不改记录' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    [void](Invoke-Down $cfg $true)
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-Equal 1 (Read-StackState $cfg).Count '记录还在'
    Assert-Match (Get-Said) '会停 后端：进程 400' '列出了会停谁'
}

Test 'Stop-VerifiedProcess：启动时间对不上就不动' {
    Reset-FakeWorld
    Add-FakeProc 800 1 $exeBackend 10
    $wrong = [string]($script:FakeT0.AddSeconds(100).ToUniversalTime().Ticks)
    Assert-True (-not (Stop-VerifiedProcess 800 $wrong)) '返回「没停」'
    Assert-Equal 0 $script:FakeKilled.Count '没有动它'
    Assert-True (Stop-VerifiedProcess 800 (Get-ProcTicks (Get-ProcInfo 800))) '对得上才停'
    Assert-Equal @(800) @($script:FakeKilled) '停了'
}

Test '认领：后端在向导页里重启之后，进程号变了' {
    Reset-FakeWorld
    if (Test-Path -LiteralPath $cfg.StateFile) { Remove-Item -LiteralPath $cfg.StateFile -Force }
    Add-FakeProc 500 1 $exeBackend 10 @(51022)
    $record = New-FakeRecord 500 51022
    $script:FakeProcs.Remove(500); $script:FakeListen = @{}       # 旧进程退出了
    Add-FakeProc 501 500 $exeBackend 40 @(51022)                  # 它拉起的新进程接着听同一个端口
    $state = @{ backend = $record }
    $adopted = Invoke-Adopt $cfg $state $false
    Assert-Equal @('backend') @($adopted) '认领了后端'
    Assert-Equal 501 $state['backend'].pid '记录换成了新进程'
    Assert-Equal 'old.log' $state['backend'].log '日志接着用原来那个文件'
    Assert-True ([bool]$state['backend'].adopted) '记下了是认领来的'
    Assert-True ((Test-RecordedProcess $cfg 'backend' $state['backend']).Ours) '新记录核对得过'
    Assert-Match (Get-Said) '认领了后端的进程 501' '说了一声'
    Assert-True (-not (Test-Path -LiteralPath $cfg.StateFile)) '不占锁的命令（status、smoke…）不写状态文件'
}

Test '认领：没有记录的也认（上一次 up 没跑完），并且写回状态文件' {
    Reset-FakeWorld
    if (Test-Path -LiteralPath $cfg.StateFile) { Remove-Item -LiteralPath $cfg.StateFile -Force }
    Add-FakeProc 20 1 $exeDb 5 @(51009)
    $state = @{}
    $adopted = Invoke-Adopt $cfg $state $true
    Assert-Equal @('database') @($adopted) '认领了数据库'
    Assert-Equal 20 (Read-StackState $cfg)['database'].pid '写回了状态文件'
}

Test '认领：不该认的都不认' {
    Reset-FakeWorld
    Add-FakeProc 600 1 'D:\elsewhere\bin\aios-database.exe' 10 @(51022)     # 占着本套端口，但程序不在运行目录里
    $state = @{}
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '程序不在运行目录里：不认'

    Reset-FakeWorld
    Add-FakeProc 601 1 $exeBackend 10                                        # 程序对，但没占本套端口
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '没占本套的端口：不认'
    Assert-Equal @(601) @(Get-ServiceProgramProcs $cfg 'backend' | ForEach-Object { $_.ProcessId }) '但看得见它在跑'

    Reset-FakeWorld
    Add-FakeProc 602 1 '' 10 @(51022)                                        # 读不到程序路径
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '读不到程序路径：不认'
    Assert-Equal 602 (Find-AdoptableProcess $cfg 'backend').UnreadablePid '但记下了有这么一个读不到的'

    Reset-FakeWorld
    Add-FakeProc 603 1 $exeBackend 10 @(51022, 8009)                         # 占着受保护的端口
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '占着受保护的端口：不认'

    Reset-FakeWorld
    Add-FakeProc 604 1 $exeDb 10 @(51022)                                    # 运行目录里的另一个程序占了后端的端口
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false @('backend')).Count '程序不是这个服务的：不认'
}

Test '认领：记录还有效、或者只是读不到路径的，不换' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    $state = @{ backend = (New-FakeRecord 400 51022) }
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '记录有效：不动'
    Assert-Equal 400 $state['backend'].pid '还是原来那个'
    $script:FakeProcs[400].ExecutablePath = ''                                # 这个窗口读不到它的路径了（权限）
    Assert-Equal 0 @(Invoke-Adopt $cfg $state $false).Count '读不到路径：不动'
    Assert-Equal 400 $state['backend'].pid '记录留着'
}

Test '认领：nginx 的监听挂在工作进程名下时，认领主进程' {
    Reset-FakeWorld
    Add-FakeProc 690 1 $exeCmd 9
    Add-FakeProc 700 690 $exeNginx 10
    Add-FakeProc 701 700 $exeNginx 11 @(51100)
    $state = @{}
    [void](Invoke-Adopt $cfg $state $false)
    Assert-Equal 700 $state['frontend'].pid '认领的是主进程'

    Reset-FakeWorld
    Add-FakeProc 711 710 $exeNginx 11 @(51100)
    Add-FakeProc 710 1 $exeNginx 500                                          # 「父进程」比它还晚创建：那个进程号是后来重用的
    $state = @{}
    [void](Invoke-Adopt $cfg $state $false)
    Assert-Equal 711 $state['frontend'].pid '不往上认一个假的父进程'
}

Test 'down：认领来的进程照常停；刚重启、还没占上端口的后端等它占上再停' {
    Reset-FakeWorld
    Add-FakeProc 500 1 $exeBackend 10 @(51022)
    $record = New-FakeRecord 500 51022
    $script:FakeProcs.Remove(500); $script:FakeListen = @{}
    Add-FakeProc 501 500 $exeBackend 40                                       # 新进程在跑，还没占上端口
    Save-StackState $cfg @{ backend = $record }
    $script:FakeSleeps = 0
    $script:FakeOnSleep = { $script:FakeSleeps += 1; if ($script:FakeSleeps -eq 2) { $script:FakeListen[51022] = @(501) } }
    $code = Invoke-Down $cfg $false
    Assert-Equal @(501) @($script:FakeKilled) '停的是新进程'
    Assert-Equal 0 $code '退出码 0'
    Assert-Equal 0 (Read-StackState $cfg).Count '记录清掉了'
}

Test 'down：运行目录里的程序还有进程在跑、又认领不了——说出来，退出码 1' {
    Reset-FakeWorld
    if (Test-Path -LiteralPath $cfg.StateFile) { Remove-Item -LiteralPath $cfg.StateFile -Force }
    Add-FakeProc 601 1 $exeBackend 10                                         # 在跑，但一直不占本套的端口
    $code = Invoke-Down $cfg $false
    Assert-Equal 1 $code '退出码 1'
    Assert-Equal 0 $script:FakeKilled.Count '没有动它'
    Assert-Match (Get-Said) '运行目录里的程序还有进程在跑' '说了'
}

Test '读不到程序路径时（起它的窗口权限更高）不说成「不是本套起的」' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    $state = @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeProcs[400].ExecutablePath = ''
    $pre = Invoke-Preflight $cfg $state
    $texts = (@($pre.Problems | ForEach-Object { "$($_.What)｜$($_.Result)｜$($_.Next)" }) -join "`n")
    Assert-Match $texts '后端 在跑，但这个窗口核对不了它' '预检说的是核对不了'
    Assert-NotMatch $texts '它不是本套起的' '预检没有说它不是本套起的'
    $info = Get-ServiceState $cfg $state 'backend'
    Assert-True $info.Unreadable '状态里标了读不到'
    Assert-Match $info.Next '同样的方式打开窗口' '下一步说的是换同样权限的窗口'
    Assert-NotMatch $info.Reason '不是本套' '没有说它不是本套的'

    $info = Get-ServiceState $cfg @{} 'backend'                               # 连记录都没有、占用者又读不到路径
    Assert-Match $info.Reason '判断不了它是不是本套的' '说的是判断不了'
    Assert-NotMatch $info.Reason '被别的进程占着' '没有一口咬定是别人的'
}

Test 'status：rs-server 因为库版本没起时，说的是哪种情况——等初始化的那一种要看后端闲不闲' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    $state = @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeKind = 'awaiting-model'
    $script:FakeIdle = $true
    function Get-StorageVersionState($Cfg) { return @{ Kind = $script:FakeKind; Text = '库里的版本记录：还没有；程序要的：1'; Idle = $false; Watch = @(7352); Stored = $null; Expected = 1 } }
    function Test-BackendIdle($Cfg) { return $script:FakeIdle }
    $info = Get-ServiceState $cfg $state 'rs_server'
    Assert-Equal 'waiting' $info.State 'rs-server 是「等待」'
    Assert-True $info.Sv.Idle '后端闲着：标出来了'
    Assert-Match $info.Next 'stack init' '下一步是先初始化'

    $script:FakeIdle = $false
    $info = Get-ServiceState $cfg $state 'rs_server'
    Assert-True (-not $info.Sv.Idle) '后端在忙：没有标「闲着」'

    $script:FakeKind = 'ok'
    $info = Get-ServiceState $cfg $state 'rs_server'
    Assert-Equal 'stopped' $info.State '库版本一致、只是还没起：就是「没起」'

    $script:FakeKind = 'no-record'
    $info = Get-ServiceState $cfg $state 'rs_server'
    Assert-Equal 'waiting' $info.State '有数据没记录：也是「等待」，并且带着归类给后面说明用'
    Assert-Equal 'no-record' $info.Sv.Kind '归类带出来了'
}

# ───────────── 停止：先请它自己退出；后端在忙就不停 ─────────────

Test 'down：数据库收到 Ctrl+C 自己退出——不直接结束；名单里是它自己和外面那层 cmd' {
    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9                                   # 外面包的那层 cmd
    Add-FakeProc 31 30 $exeDb 10 @(51009)                         # 数据库
    Save-StackState $cfg @{ database = (New-FakeRecord 31 51009 30) }
    $code = Invoke-Down $cfg $false
    Assert-Equal 0 $code '退出码 0'
    Assert-Equal 0 $script:FakeKilled.Count '一个都没有直接结束'
    Assert-Equal 1 $script:FakeCtrlCalls.Count '发了一次 Ctrl+C'
    Assert-Equal 31 $script:FakeCtrlCalls[0].Pid '发给的是数据库'
    Assert-Equal (Get-ProcTicks ([pscustomobject]@{ CreationDate = $script:FakeT0.AddSeconds(10) })) $script:FakeCtrlCalls[0].Ticks '带着它的启动时间（小进程动手前再核对一次）'
    Assert-Equal @(30, 31) @($script:FakeCtrlCalls[0].Allowed | Sort-Object) '名单：数据库自己 + 外面那层 cmd'
    Assert-Match (Get-Said) '已停 数据库：进程 31.*它自己退出的：Ctrl\+C' '说了是它自己退出的'
    Assert-Equal 0 (Read-StackState $cfg).Count '记录清掉了'
}

Test 'down：数据库等了 stop_sec 还没退——改为直接结束，并说出来' {
    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9
    Add-FakeProc 31 30 $exeDb 10 @(51009)
    Save-StackState $cfg @{ database = (New-FakeRecord 31 51009 30) }
    $script:FakeCtrlMode = 'hang'
    $before = $script:FakeNow
    $code = Invoke-Down $cfg $false
    Assert-Equal 0 $code '退出码 0'
    Assert-Equal @(30, 31) @($script:FakeKilled | Sort-Object) '直接结束了数据库，连带外面那层 cmd'
    Assert-True ((($script:FakeNow - $before).TotalSeconds) -ge 30) '等够了 30 秒（假的钟）'
    Assert-Match (Get-Said) '还在等（最多 30 秒）' '等的时候说了一声'
    Assert-Match (Get-Said) '请它自己退出（Ctrl\+C）没有成——等了 30 秒它还没有退出。改为直接结束' '说了为什么改成直接结束'
    Assert-Match (Get-Said) '已停 数据库：进程 31.*直接结束的：请它自己退出（Ctrl\+C）没有成' '收尾那一句也说清楚了'
}

Test 'down：数据库的控制台上还有别的进程——不发 Ctrl+C，只直接结束这一个' {
    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9
    Add-FakeProc 31 30 $exeDb 10 @(51009)
    Save-StackState $cfg @{ database = (New-FakeRecord 31 51009 30) }
    $script:FakeCtrlMode = 'refuse'
    [void](Invoke-Down $cfg $false)
    Assert-True (@($script:FakeKilled) -contains 31) '数据库是直接结束的'
    Assert-Match (Get-Said) '它的控制台上还有别的进程' '说了为什么没发'
}

Test '包着服务的那层 cmd：记录里有的核对后才用；认领来的看父进程是不是那层 cmd' {
    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9
    Add-FakeProc 31 30 $exeDb 10 @(51009)
    $record = New-FakeRecord 31 51009 30
    Assert-Equal 30 (Get-WrapperProc $record (Get-ProcInfo 31)).ProcessId '记录里的那层 cmd'
    $script:FakeProcs.Remove(30)
    Add-FakeProc 30 1 $exeCmd 500                                 # 同一个进程号，后来分给了另一个 cmd
    Assert-True ($null -eq (Get-WrapperProc $record (Get-ProcInfo 31))) '启动时间对不上：不认'

    Reset-FakeWorld
    Add-FakeProc 40 1 $exeCmd 9
    $script:FakeProcs[40].CommandLine = 'C:\Windows\System32\cmd.exe /d /s /c ""' + $exeDb + '" "start" <NUL 1>>"db.log" 2>&1"'
    Add-FakeProc 41 40 $exeDb 10 @(51009)
    $adopted = New-FakeRecord 41 51009 0                           # 认领来的：记录里没有那层 cmd
    Assert-Equal 40 (Get-WrapperProc $adopted (Get-ProcInfo 41)).ProcessId '父进程是 cmd、命令行里就是这个程序：认'
    $script:FakeProcs[40].CommandLine = 'C:\Windows\System32\cmd.exe'
    Assert-True ($null -eq (Get-WrapperProc $adopted (Get-ProcInfo 41))) '父进程是别人开的命令行窗口（命令行里没有这个程序）：不认'
    $script:FakeProcs[40].CommandLine = 'cmd /c "' + $exeDb + '"'
    $script:FakeProcs[40].CreationDate = $script:FakeT0.AddSeconds(900)
    Assert-True ($null -eq (Get-WrapperProc $adopted (Get-ProcInfo 41))) '「父进程」比它还晚创建（进程号重用）：不认'
    $script:FakeProcs.Remove(40)
    Add-FakeProc 40 1 'C:\Windows\explorer.exe' 9
    $script:FakeProcs[40].CommandLine = 'explorer "' + $exeDb + '"'
    Assert-True ($null -eq (Get-WrapperProc $adopted (Get-ProcInfo 41))) '父进程不是 cmd：不认'

    # 认领来的数据库：名单里带上认出来的那层 cmd
    Reset-FakeWorld
    Add-FakeProc 40 1 $exeCmd 9
    $script:FakeProcs[40].CommandLine = 'cmd.exe /d /s /c ""' + $exeDb + '" "start""'
    Add-FakeProc 41 40 $exeDb 10 @(51009)
    Save-StackState $cfg @{ database = (New-FakeRecord 41 51009 0) }
    [void](Invoke-Down $cfg $false)
    Assert-Equal @(40, 41) @($script:FakeCtrlCalls[0].Allowed | Sort-Object) '认领来的数据库：名单里有认出来的那层 cmd'
    Assert-Equal 0 $script:FakeKilled.Count '它自己退出了，没有直接结束谁'
}

Test 'down：nginx 用 -s quit 自己退出；它自己记的进程号对不上、或者那条命令报错，就直接结束' {
    $pidFile = Join-Path $cfg.Frontend.WorkDir 'logs\nginx.pid'
    try {
        Reset-FakeWorld
        Add-FakeProc 90 1 $exeCmd 99
        Add-FakeProc 100 90 $exeNginx 100 @(51100)
        Add-FakeProc 101 100 $exeNginx 101
        Write-TestFile $pidFile "100`r`n"
        Save-StackState $cfg @{ frontend = (New-FakeRecord 100 51100 90) }
        $code = Invoke-Down $cfg $false
        Assert-Equal 0 $code '退出码 0'
        Assert-Equal 0 $script:FakeKilled.Count '一个都没有直接结束'
        Assert-Equal 1 $script:FakeCaptureCalls.Count '跑了一次 nginx -s quit'
        Assert-Equal $exeNginx $script:FakeCaptureCalls[0].Exe '跑的是记录里那个程序'
        Assert-Match $script:FakeCaptureCalls[0].ArgLine ([regex]::Escape('"-p" "' + $cfg.Frontend.WorkDir.Replace('\', '/') + '/" "-c" "conf/nginx.conf" -s quit')) '参数：同一个运行目录、同一份配置，加 -s quit'
        Assert-Match (Get-Said) '已停 前端：进程 100.*它自己退出的：nginx -s quit' '说了是它自己退出的'

        Reset-FakeWorld
        Add-FakeProc 90 1 $exeCmd 99
        Add-FakeProc 100 90 $exeNginx 100 @(51100)
        Add-FakeProc 101 100 $exeNginx 101
        Write-TestFile $pidFile '555'                             # nginx 自己记的是另一个进程号
        Save-StackState $cfg @{ frontend = (New-FakeRecord 100 51100 90) }
        [void](Invoke-Down $cfg $false)
        Assert-Equal 0 $script:FakeCaptureCalls.Count '没有跑 nginx -s quit'
        Assert-Equal @(90, 100, 101) @($script:FakeKilled | Sort-Object) '直接结束：主进程、工作进程、外面那层 cmd'
        Assert-Match (Get-Said) '它自己记的进程号.*不是本套记着的 100' '说了为什么'

        Reset-FakeWorld
        Add-FakeProc 90 1 $exeCmd 99
        Add-FakeProc 100 90 $exeNginx 100 @(51100)
        Write-TestFile $pidFile '100'
        Save-StackState $cfg @{ frontend = (New-FakeRecord 100 51100 90) }
        $script:FakeQuitMode = 'fail'
        [void](Invoke-Down $cfg $false)
        Assert-True (@($script:FakeKilled) -contains 100) '那条命令报错：直接结束'
        Assert-Match (Get-Said) 'nginx -s quit 没有成功' '说了为什么'
    } finally {
        if (Test-Path -LiteralPath $pidFile) { Remove-Item -LiteralPath $pidFile -Force }
    }
}

Test 'down：后端正在忙——什么都不停，退出码 6；-WhatIf 只提醒；-Force 照停' {
    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9
    Add-FakeProc 31 30 $exeDb 10 @(51009)
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ database = (New-FakeRecord 31 51009 30); backend = (New-FakeRecord 400 51022) }
    $script:FakeBusy = @{ Known = $true; Busy = $true; Reasons = @('队列里正在跑：库 7352（apply_window，parse）', '生成好的模型还有 12 个根没写进数据库（写后台没清空）') }
    $code = Invoke-Down $cfg $false
    Assert-Equal 6 $code '退出码 6'
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-Equal 0 $script:FakeCtrlCalls.Count '也没有请数据库退出'
    Assert-Equal 2 (Read-StackState $cfg).Count '记录都还在'
    Assert-Match (Get-Said) '后端正在忙，这一次什么都没有停' '说了没停'
    Assert-Match (Get-Said) '库 7352（apply_window，parse）' '说了它手上的活'
    Assert-Match (Get-Said) '12 个根没写进数据库' '说了写后台'
    Assert-Match (Get-Said) 'stack down -Force' '说了确实要停怎么办'

    $script:FakeSaid.Clear()
    $code = Invoke-Down $cfg $true
    Assert-Equal 0 $code '-WhatIf 的退出码 0'
    Assert-Equal 0 $script:FakeKilled.Count '-WhatIf 不停'
    Assert-Match (Get-Said) '要加 -Force 才停' '-WhatIf 提醒了'
    Assert-Match (Get-Said) '会停 后端：进程 400.*直接结束（这个程序没有自己退出的入口）' '-WhatIf 照样列出会停谁、怎么停'
    Assert-Match (Get-Said) '会停 数据库：进程 31.*先发 Ctrl\+C 请它自己退出，最多等 30 秒' '-WhatIf 说了数据库怎么停'

    $script:FakeSaid.Clear()
    $code = Invoke-Down $cfg $false $true
    Assert-Equal 0 $code '-Force 的退出码 0'
    Assert-Equal @(400) @($script:FakeKilled) '-Force：后端直接结束，数据库是自己退出的'
    Assert-Equal 1 $script:FakeCtrlCalls.Count '数据库照样先请它自己退出'
    Assert-Match (Get-Said) '后端正在忙，按 -Force 照停' '说了是按 -Force 停的'
    Assert-Equal 0 (Read-StackState $cfg).Count '记录清掉了'
}

Test 'down：问不到后端忙不忙——也不停，退出码 6；本套没有后端在跑时不问' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeBusy = @{ Known = $false; Busy = $false; Reasons = @() }
    $code = Invoke-Down $cfg $false
    Assert-Equal 6 $code '退出码 6'
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-Match (Get-Said) '接口没有应答' '说了是问不到'
    $code = Invoke-Down $cfg $false $true
    Assert-Equal 0 $code '-Force：照停'
    Assert-Equal @(400) @($script:FakeKilled) '停了后端'

    Reset-FakeWorld
    Add-FakeProc 30 1 $exeCmd 9
    Add-FakeProc 31 30 $exeDb 10 @(51009)
    Save-StackState $cfg @{ database = (New-FakeRecord 31 51009 30) }
    $script:FakeBusy = @{ Known = $false; Busy = $false; Reasons = @() }
    $code = Invoke-Down $cfg $false
    Assert-Equal 0 $code '只有数据库在跑：照常停'
    Assert-Equal 0 $script:FakeBusyAsked '没有去问后端'
}

Test '护栏越线的自动收摊：后端在忙也收' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeBusy = @{ Known = $true; Busy = $true; Reasons = @('队列里正在跑：库 7352') }
    $cfg.Guard.AutoDown = $true
    try {
        [void](Assert-StackError { Stop-OnGuard $cfg '测试：越线了' } 3 '' '越线之后以退出码 3 收场')
        Assert-Equal @(400) @($script:FakeKilled) '后端收掉了'
        Assert-Match (Get-Said) '后端手上有没有活都收' '说了为什么不看忙不忙'
    } finally {
        $cfg.Guard.AutoDown = $false
        if ($null -ne $script:LockStream) { $script:LockStream.Dispose(); $script:LockStream = $null }
    }
}

Test 'down：后端在忙、库里的版本记录又还没写上——拒绝时多说一段（这时强停，记录以后不会补写）' {
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeBusy = @{ Known = $true; Busy = $true; Reasons = @('队列里正在跑：库 8190（apply_window，meta）'); NoRecord = $true }
    $code = Invoke-Down $cfg $false
    Assert-Equal 6 $code '退出码 6'
    Assert-Equal 0 $script:FakeKilled.Count '一个都没停'
    Assert-Match (Get-Said) '库里的版本记录还没有写上' '说了版本记录还没写'
    Assert-Match (Get-Said) 'rs-server 会一直起不来' '说了后果'
    $script:FakeSaid.Clear()
    $code = Invoke-Down $cfg $true
    Assert-Match (Get-Said) '库里的版本记录还没有写上' '-WhatIf 也说'
    $script:FakeSaid.Clear()
    $code = Invoke-Down $cfg $false $true
    Assert-Equal 0 $code '-Force 照停'
    Assert-Match (Get-Said) '后端多半不会再补写它' '-Force 时提醒一句'

    # 版本记录已经在的时候不说这一段
    Reset-FakeWorld
    Add-FakeProc 400 1 $exeBackend 50 @(51022)
    Save-StackState $cfg @{ backend = (New-FakeRecord 400 51022) }
    $script:FakeBusy = @{ Known = $true; Busy = $true; Reasons = @('队列里正在跑：库 7352'); NoRecord = $false }
    [void](Invoke-Down $cfg $false)
    Assert-NotMatch (Get-Said) '版本记录还没有写上' '记录在的时候不提'
}