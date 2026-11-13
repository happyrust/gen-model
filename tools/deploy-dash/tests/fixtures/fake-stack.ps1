# 测试用的假 stack.ps1（deploy-dash-fake-stack）：tests\mock-root.ps1 把它装进 mock 根目录，
# 让代理的单服务「停 → 换 → 起」在 mock 下真走一遍。不起停任何进程，
# 只在 <根>\run\fake-stack\running.txt 里记哪些服务算在跑；没有这个文件就当四个都在跑。
#
# 两种用法和真的 stack.ps1 一样：
#   powershell -File stack.ps1 up|down [-Config <stack.toml>]   命令行：全起 / 全停，退出码 0
#   . stack.ps1 __lib                                           当函数库加载（svc_upgrade.rs 停、起单个服务就这么用）
# 函数只假到 svc_upgrade.rs 那两段脚本用得着的程度；那边多用了函数，这里跟着补。
# 要看「新版本起不来 → 自动换回」：在 <根>\run\fake-stack\fail-ready.txt 里写服务名（一行一个），等就绪那步回退出码 4。
# 代理用 Windows PowerShell 5.1 跑它：文件存成带 BOM 的 UTF-8，输出只用 ASCII。
param([Parameter(Position = 0)][string]$Command = 'help', [string]$Config = '', [switch]$NoInit)

$FakeServices = @('database', 'backend', 'rs_server', 'frontend')

# 函数的返回值会被调用方接住，日志直接写标准输出
function Write-Fake([string]$Text) { [Console]::Out.WriteLine("[fake-stack] $Text") }

function Get-StackConfig([string]$Path) {
    $root = Split-Path -Parent ([IO.Path]::GetFullPath($Path))
    $run = Join-Path $root 'run\fake-stack'
    [pscustomobject]@{
        Root     = $root
        RunDir   = $run
        LogsDir  = Join-Path $run 'logs'
        Guard    = [pscustomobject]@{ ProtectPorts = @() }
        Database = [pscustomobject]@{ DataDir = Join-Path $run 'db' }
        Frontend = [pscustomobject]@{ Kind = 'static' }
    }
}
function New-Dir([string]$Path) { if ($Path -and -not (Test-Path -LiteralPath $Path)) { [void](New-Item -ItemType Directory -Force -Path $Path) } }
function Say([string]$Text, [string]$Color = '') { [Console]::Out.WriteLine($Text) }
function Stop-Stack([string]$Message, [int]$Code = 1) {
    $ex = New-Object System.Exception $Message
    $ex.Data['StackExitCode'] = $Code
    throw $ex
}
function Enter-StackLock($Cfg) { }

function Read-StackState($Cfg) {
    $file = Join-Path $Cfg.RunDir 'running.txt'
    $names = $FakeServices
    if (Test-Path -LiteralPath $file) { $names = @([IO.File]::ReadAllText($file) -split "`r?`n" | Where-Object { $_.Trim() }) }
    $state = @{}
    foreach ($n in $names) { $state[$n.Trim()] = @{ pid = 0; startTicks = 0; wrapperPid = 0; wrapperTicks = 0 } }
    $state
}
function Save-StackState($Cfg, $State) {
    New-Dir $Cfg.RunDir
    $names = @($FakeServices | Where-Object { $State.ContainsKey($_) })
    [IO.File]::WriteAllText((Join-Path $Cfg.RunDir 'running.txt'), ($names -join "`n"))
}

# 停
function Invoke-Adopt($Cfg, $State, $Quiet, $Names) { $false }
function Test-RecordedProcess($Cfg, $Name, $Record) { [pscustomobject]@{ Ours = ($null -ne $Record); Unreadable = $false; Why = ''; Proc = $null } }
function Get-BackendBusy($Cfg) { [pscustomobject]@{ Known = $true; Busy = $false; Reasons = @() } }
function Get-ListenTable([switch]$Fresh) { @{} }
function Get-ChildProcs($ParentId, $Ticks) { }
function Request-GracefulStop($Cfg, $Name, $Record, $Proc) {
    Write-Fake "stop $Name"
    [pscustomobject]@{ Exited = $true }
}
function Stop-VerifiedProcess($ProcId, $Ticks) { $true }
function Get-ProcInfo($ProcId) { $null }
function Test-SameStart($Ticks, $Proc) { $false }

# 起
function Initialize-BackendFiles($Cfg) { }
function Initialize-RsServerFiles($Cfg) { }
function Initialize-FrontendFiles($Cfg) { }
function Get-ServiceState($Cfg, $State, $Name) { [pscustomobject]@{ State = $(if ($State.ContainsKey($Name)) { 'ok' } else { 'stopped' }) } }
function Start-StackService($Cfg, $State, $Name, $Index, $Total) {
    Write-Fake "start $Name"
    $State[$Name] = @{ pid = 0; startTicks = 0; wrapperPid = 0; wrapperTicks = 0 }
    Save-StackState $Cfg $State
}
function Wait-ServiceReady($Cfg, $State, $Name, $Index, $Total) {
    $fail = Join-Path $Cfg.RunDir 'fail-ready.txt'
    if ((Test-Path -LiteralPath $fail) -and (@([IO.File]::ReadAllText($fail) -split "`r?`n" | ForEach-Object { $_.Trim() }) -contains $Name)) {
        Stop-Stack "fake: $Name not ready (fail-ready.txt)" 4
    }
    Write-Fake "ready $Name"
}
function Get-ServiceTimeout($Cfg, $Name) { 5 }
function Get-FrontendUrl($Cfg) { 'http://127.0.0.1:9' }
function Invoke-StackHttp($Method, $Url, $Body, $TimeoutSec) { [pscustomobject]@{ Ok = $true; Body = '<div id="app"></div>' } }
function Update-PlantUiWebConfig($Cfg) { }

if ($Command -eq '__lib') { return }

$cfg = Get-StackConfig $(if ($Config) { $Config } else { Join-Path $PSScriptRoot 'stack.toml' })
Write-Output "fake stack: $Command"
$state = Read-StackState $cfg
if ($Command -eq 'up') { foreach ($n in $FakeServices) { $state[$n] = @{ pid = 0 } }; Save-StackState $cfg $state }
if ($Command -eq 'down') { $state.Clear(); Save-StackState $cfg $state }
exit 0
