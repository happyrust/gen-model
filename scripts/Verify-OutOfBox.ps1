<#
.SYNOPSIS
    「开箱即用」验收：只起 gen-model 一个进程、**一台数据库都不拉**（store_mode = mem），
    不预解析、不预生成；模型等 plant-ui 点眼睛时按需生成。

.DESCRIPTION
    这个脚本只做**能自动判的那一半**：起服务、等它就绪、把 /health 上该看的几栏打出来、
    确认它没有偷偷拉起任何 surreal 子进程。剩下一半（点树、点眼睛、看三维出没出来）只有人
    能做，脚本跑完会把该点哪几下、该看哪几行原样列出来。

    口径（2026-09-13，ADR-074）：存储只有 mem / rocksdb 两档，服务在任何一档都不拉起数据库。
    mem 档的库在进程里：没有端口、活不过重启，外部 SQL / rvm_verify / plant-ui「库供数」
    读面都连不到它——这是这一档的定义，不是故障。

    **它不改任何配置。** 形态全靠环境变量给：
      AIOS_STORE_MODE=mem           库在进程里，不拉任何子进程
      AIOS_STARTUP_AUTORUN=0        启动不自动干活——这一条是「不预解析」的开关
      AIOS_DATA_READ_MODE 不设      默认就是 direct（树与模型从库文件读）

.PARAMETER Exe
    服务二进制。默认 D:\Rust\target\release\aios-database.exe（`cargo build --release --bin aios-database` 的产物）。

.PARAMETER Config
    DB_OPTION_FILE 的值（不带 .toml）。默认用仓根那份 DbOption。

.PARAMETER WatchDbnums
    收窄监听范围，逗号分隔（走 `serve --watch-dbnum`，压过配置里的 `watch_dbnums`）。
    **第一次跑务必给**：不限定的话启动会去扫全部候选库。

.PARAMETER KeepRunning
    验完不杀服务，留着让人接着点 plant-ui。默认跑完就收。

.EXAMPLE
    # 第一次：限定到一个设计库，跑完自动收
    ./scripts/Verify-OutOfBox.ps1 -WatchDbnums 7997

.EXAMPLE
    # 留着接着用 plant-ui 点
    ./scripts/Verify-OutOfBox.ps1 -WatchDbnums 7997 -KeepRunning
#>
[CmdletBinding()]
param(
    [string]$Exe = 'D:\Rust\target\release\aios-database.exe',
    [string]$Config = 'DbOption',
    [string]$WatchDbnums,
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo ('output\out-of-box\{0}' -f (Get-Date -Format yyyyMMdd-HHmmss))
New-Item -ItemType Directory -Force $out | Out-Null

function Read-ConfigValue {
    param([string]$Text, [string]$Key)
    if ($Text -match "(?m)^\s*$Key\s*=\s*`"?([^`"\r\n#]+)`"?") { return $Matches[1].Trim() }
    return $null
}

# ── 0. 读配置：只要 HTTP 面的地址；v_ip / v_port 在 mem 档根本不用 ─────────────────
$configPath = Join-Path $repo ($Config + '.toml')
if (-not (Test-Path -LiteralPath $configPath)) { throw "找不到配置 $configPath" }
$configText = [IO.File]::ReadAllText($configPath)
$httpAddr = Read-ConfigValue $configText 'http_api_addr'
if (-not $httpAddr) { throw "$configPath 没有 http_api_addr，服务不会起 HTTP 面，验不了" }
$declaredMode = Read-ConfigValue $configText 'store_mode'
if ($declaredMode -and $declaredMode -ne 'mem') {
    Write-Host "注意：$Config 里写了 store_mode = $declaredMode，本次用环境变量压成 mem。" -ForegroundColor Yellow
}

if (-not (Test-Path -LiteralPath $Exe)) {
    throw "服务二进制不在：$Exe`n先跑 cargo build --release --bin aios-database"
}

# ── 1. 起服务 ─────────────────────────────────────────────────────────────────
$env:DB_OPTION_FILE = $Config
$env:AIOS_STORE_MODE = 'mem'
# 「不预解析」就是这一条：启动只做让库能用的幂等自愈，不消费队列、不做全量房间重建。
$env:AIOS_STARTUP_AUTORUN = '0'
$env:RUST_MIN_STACK = '134217728'
$env:RUST_BACKTRACE = '1'

# 监听限定只能从命令行来（`watch_scope` 有意不给它环境变量入口：它是调试开关，
# 不该躺在环境里跨会话活着）。
$svcArgs = @()
if ($WatchDbnums) { $svcArgs = @('serve', '--watch-dbnum', $WatchDbnums) }

$stdout = Join-Path $out 'aios-database.stdout.log'
$stderr = Join-Path $out 'aios-database.stderr.log'
Write-Host "起服务：$Exe $($svcArgs -join ' ')（配置 $Config，AIOS_STORE_MODE=mem，autorun=0）"
# 起之前记下机器上已有的 surreal 进程：之后多出来的任何一个都算服务违规拉起。
$surrealBefore = @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" -ErrorAction SilentlyContinue |
    Select-Object -ExpandProperty ProcessId)
$startArgs = @{
    FilePath               = $Exe
    WorkingDirectory       = $repo
    WindowStyle            = 'Hidden'
    PassThru               = $true
    RedirectStandardOutput = $stdout
    RedirectStandardError  = $stderr
}
if ($svcArgs.Count) { $startArgs['ArgumentList'] = $svcArgs }
$svc = Start-Process @startArgs

$healthUrl = "http://$httpAddr/api/v1/health"
$ready = $false
foreach ($null in 1..300) {
    Start-Sleep -Seconds 1
    if ($svc.HasExited) { break }
    try {
        $health = Invoke-RestMethod $healthUrl -TimeoutSec 2
        if ($health) { $ready = $true; break }
    }
    catch {}
}

if (-not $ready) {
    Write-Host '--- stdout 尾巴 ---' -ForegroundColor Red
    Get-Content $stdout -Tail 60 -ErrorAction SilentlyContinue
    Write-Host '--- stderr 尾巴 ---' -ForegroundColor Red
    Get-Content $stderr -Tail 40 -ErrorAction SilentlyContinue
    if (-not $svc.HasExited) { Stop-Process -Id $svc.Id -Force }
    throw "服务没有在 300s 内答话；日志在 $out"
}

# ── 2. 自动判得了的那几条 ─────────────────────────────────────────────────────
$health | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out 'health.json')
$store = $health.sul_db
$childStore = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($svc.Id)" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -eq 'surreal.exe' })
$surrealNew = @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessId -notin $surrealBefore })

Write-Host ''
Write-Host '=== 自动判 ===' -ForegroundColor Cyan
$checks = [ordered]@{
    '服务起来了'                          = $true
    '服务没有拉起 surreal 子进程'         = ($childStore.Count -eq 0 -and $surrealNew.Count -eq 0)
    'sul_db.connected'                    = [bool]$store.connected
    'sul_db.medium = mem'                 = ($store.medium -eq 'mem')
    'sul_db.durable = false'              = ($store.durable -eq $false)
    'sul_db.endpoint = embedded:mem'      = ($store.endpoint -eq 'embedded:mem')
    'data_face = read-through'            = ($health.data_face -eq 'read-through')
}
foreach ($k in $checks.Keys) {
    $ok = $checks[$k]
    Write-Host ("  [{0}] {1}" -f $(if ($ok) { '✓' } else { '×' }), $k) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
}
Write-Host ("  sul_db.endpoint = {0} / medium = {1} / durable = {2} / data_face = {3}" -f $store.endpoint, $store.medium, $store.durable, $health.data_face)
if ($surrealNew.Count -gt 0) { Write-Host ("  [!] 起服务之后机器上多出 surreal 进程 pid = {0}" -f ($surrealNew.ProcessId -join ',')) -ForegroundColor Red }

# 树走文件，不吃库——冷库上它就该立刻有内容。这是「不预解析也能用」的第一块证据。
try {
    $roots = Invoke-RestMethod "http://$httpAddr/api/v1/tree/roots" -TimeoutSec 20
    $rootCount = @($roots.nodes ?? $roots).Count
    Write-Host ("  [{0}] /api/v1/tree/roots 回了 {1} 个根（树走文件，不等灌库）" -f $(if ($rootCount -gt 0) { '✓' } else { '×' }), $rootCount) `
        -ForegroundColor $(if ($rootCount -gt 0) { 'Green' } else { 'Red' })
    $roots | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $out 'tree-roots.json')
}
catch {
    Write-Host "  [×] /api/v1/tree/roots 失败：$_" -ForegroundColor Red
}

# ── 3. 只有人能做的那一半 ─────────────────────────────────────────────────────
Write-Host ''
Write-Host '=== 接下来只有人能做（脚本判不了） ===' -ForegroundColor Cyan
Write-Host @"
  1. 起 plant-ui，模型服务地址填 http://$httpAddr（mem 档没有数据库端口，plant-ui 全走服务 API；
     它的「库供数」读面在这一档会明确报「不可直连」，那是预期，不是故障）
  2. 树应当**立刻**能展开（走文件，不等灌库）
  3. 找一个 EQUI 或 BOX，点它的眼睛 → 服务按需生成 → 三维出现
  4. 再找一根 PIPE / BRAN 点眼睛 —— 这一格是本次验收真正的靶子，见下面「已知会踩的两处」
  5. Ctrl+C 掉服务，确认 tasklist 里没有 surreal——本来就不该有

  已知会踩的两处（都是「零解析」的直接后果，不是本轮改动的回归）：
    * `pe` 表是空的 → plant-ui 查隐式直管那条子查询取 `in.owner`，没有 pe 行就是 NONE，
      而 `GeomInstQuery.owner` 不是 Option → **一根 BRAN 的直管可能整批解不出来**。
      EQUI / BOX 这类走平表快路（owner 从 anc[1] 还原）不受影响。
    * `MDB` / `WORL` 表是空的 → plant-ui 的项目标识里 DESI 库名单为空，
      「模型更新」的本期范围会是空的。树与按需生成不受影响。
  两处都靠「让数据那一半也灌进来」解决：去掉 AIOS_STARTUP_AUTORUN=0 再跑一次，
  代价是每次开机一趟 DESI 基线——那个耗时正是本次要量的数。

  日志与回执：$out
"@

if ($KeepRunning) {
    Write-Host ("服务留着（pid={0}）。用完请自己 Stop-Process -Id {0}。" -f $svc.Id) -ForegroundColor Yellow
    return
}

# ── 4. 收尾：杀服务 ───────────────────────────────────────────────────────────
# mem 档没有子进程，也就没有「孤儿存储进程」这回事；强杀之后只需确认服务本身退了。
Write-Host ''
Write-Host '=== 收尾 ===' -ForegroundColor Cyan
Stop-Process -Id $svc.Id -Force
Wait-Process -Id $svc.Id -Timeout 30 -ErrorAction SilentlyContinue
$leftover = @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessId -notin $surrealBefore })
if ($leftover.Count -eq 0) {
    Write-Host '  [✓] 服务已退，机器上没有本次多出来的 surreal 进程' -ForegroundColor Green
}
else {
    Write-Host ("  [×] 服务退了但多出 surreal 进程 pid={0}——mem 档不该有任何库进程" -f ($leftover.ProcessId -join ',')) -ForegroundColor Red
}
