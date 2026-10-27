<#
.SYNOPSIS
    L2 回放（tests/increment_replay）的唯一跑法入口：独立 CARGO_TARGET_DIR、单线程、日志落盘、四类异常判定。

.DESCRIPTION
    【为什么必须用独立 CARGO_TARGET_DIR（默认 D:\Rust\target-l2-m1cwd）】
    本机常驻的开发服务 aios-database.exe 就是从默认 target（D:\Rust\target）的 debug\aios-database.exe 起的；
    Windows 锁着运行中的 exe，cargo test --test increment_replay 要重新链接这个二进制时就会报

        error: failed to remove file `D:\Rust\target\debug\aios-database.exe`
        Caused by: 拒绝访问。 (os error 5)

    并以退出码 101 退出 —— 与 L2 用例真红的退出码一模一样，只看退出码会把环境错误误读成 L2 红。
    独立 target 下有自己的一份 debug\aios-database.exe，不与常驻服务争同一个文件，并发编译 / 并发运行都成立。

    【为什么不能随手新建 target 目录】
    新目录意味着从零编译整棵依赖树，实测 D:\Rust\target-l2-verify8 从零编译崩溃 0xc0000409；旧默认值 D:\Rust\target-l2
    压根不存在。D:\Rust\target-l2-m1cwd 已实测可用（整档 1476 s 跑通）。要换目录先手工验证，再改默认值
    （本脚本与 scripts/mission-gate.ps1 的 $L2TargetDir 要一起改）。

    【判绿三条，缺一不可】
    1. 退出码 0；
    2. 整档墙钟 > -MinWallClockSec（默认 1000 s；整档实测 1476 s；单例传 0，给了 -Case 又没显式传时自动取 0）；
    3. 日志里没有 "skip increment_replay:" —— 有就是前置不齐被整档跳过的假绿（1 s 报 ok），典型原因是跨 worktree
       复用 target：二进制里嵌的 CARGO_MANIFEST_DIR 指向别的检出，仓根 DbOption.toml / bin\surreal.exe 找不到。

    【本脚本的退出码（把环境错误从 cargo 的 101 里分出来）】
    0    真绿
    2    假绿（skip increment_replay: / 墙钟低于下限）
    3    环境：os error 5（target 被占用，不是 L2 红），或 -TargetDir 不合法 / 造影子根失败
    4    环境：stage=instance-lock（被测服务抢到了仓根单实例锁；M1 已修，不该再出现）
    其它 原样透传 cargo 的退出码（101 = L2 真红或编译错误，看日志）

    产物：_runs\l2-<Owner>\logs\<stamp>-l2.log（完整 stdout+stderr）、同目录 summary.txt（每次追加一段 key=value）。
    跑前把既有 aios-database / surreal 进程清单记进日志头，只记录，绝不 kill。

.EXAMPLE
    pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1                                              # 全档
    pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -Case ams8000-equi-data-fail-park -MinWallClockSec 0
    pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -IncludeKnownRed
    pwsh -NoProfile -File scripts/replay/Run-L2Replay.ps1 -DryRun                                      # 只看将要执行什么
#>
[CmdletBinding()]
param(
    # 只跑一条用例（tests/increment_replay/cases/<name>.json 去掉 .json）→ AIOS_REPLAY_CASE
    [string]$Case = '',
    # 放行 known_red 用例 → AIOS_REPLAY_INCLUDE_KNOWN_RED=1
    [switch]$IncludeKnownRed,
    # 判绿的墙钟下限（秒）。单例请传 0；给了 -Case 又没显式传时自动取 0。
    [int]$MinWallClockSec = 1000,
    # 独立 CARGO_TARGET_DIR，只能填实测可用的目录（见顶部说明）。
    [string]$TargetDir = 'D:\Rust\target-l2-m1cwd',
    # 影子工程根（AIOS_REPLAY_PROJECT_ROOT）。缺省取已设的环境变量，再缺省 _runs\l2-<Owner>\shadowroot；不存在就自动造。
    [string]$ShadowRoot = '',
    # 运行产物归属，决定 _runs\l2-<Owner>\ 的目录名。
    [string]$Owner = $(if ($env:USERNAME) { $env:USERNAME } else { 'local' }),
    [string]$Features = 'ws,gen_model,manifold,project_hd',
    # 允许 -TargetDir 指向尚不存在的目录（从零编译，未经验证；见顶部说明）。
    [switch]$AllowNewTargetDir,
    # 只打印将要执行的环境、命令与路径，不跑、不落盘。
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
# cargo 非 0 退出是常态（L2 红 = 101），别让 pwsh 7.4+ 把它升级成异常；退出码由下面自己判。
$PSNativeCommandUseErrorActionPreference = $false

$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $repo

function Resolve-FullPath([string]$PathText) {
    # Set-Location 不改 .NET 的进程 CWD，相对路径要按 pwsh 当前位置解析。
    return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($PathText).TrimEnd('\')
}

$ownerSafe = ($Owner -replace '[\\/:*?"<>|\s]', '_')
$runDir = Join-Path $repo "_runs\l2-$ownerSafe"
$logDir = Join-Path $runDir 'logs'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$logPath = Join-Path $logDir "$stamp-l2.log"
$summaryPath = Join-Path $logDir 'summary.txt'

if (-not $ShadowRoot) {
    $ShadowRoot = if ($env:AIOS_REPLAY_PROJECT_ROOT) { $env:AIOS_REPLAY_PROJECT_ROOT } else { Join-Path $runDir 'shadowroot' }
}
$ShadowRoot = Resolve-FullPath $ShadowRoot
if ($Case -and -not $PSBoundParameters.ContainsKey('MinWallClockSec')) {
    $MinWallClockSec = 0
}
$targetFull = Resolve-FullPath $TargetDir
$cargoArgs = @('test', '--locked', '--test', 'increment_replay', '--features', $Features, '--', '--test-threads=1', '--nocapture')
$caseText = if ($Case) { $Case } else { '<全档>' }
$knownRedText = if ($IncludeKnownRed) { '1' } else { '<未设>' }

Write-Host '===== L2 replay =====' -ForegroundColor Cyan
Write-Host "  repo                          = $repo"
Write-Host "  CARGO_TARGET_DIR              = $targetFull"
Write-Host "  AIOS_REPLAY_PROJECT_ROOT      = $ShadowRoot"
Write-Host "  AIOS_REPLAY_CASE              = $caseText"
Write-Host "  AIOS_REPLAY_INCLUDE_KNOWN_RED = $knownRedText"
Write-Host "  墙钟下限                      = $MinWallClockSec s"
Write-Host "  命令                          = cargo $($cargoArgs -join ' ')"
Write-Host "  日志                          = $logPath"

# ---------------------------------------------------------------- 1. target 目录核查（只拦明确的坑，不替人新建）
$sharedTarget = 'D:\Rust\target'
if ($targetFull -ieq $sharedTarget) {
    Write-Host "拒绝：-TargetDir 指向共享 target（$sharedTarget）。常驻 aios-database.exe 独占其中的 debug\aios-database.exe，重链接必然 os error 5，退出码 101 与 L2 红无法区分。" -ForegroundColor Red
    exit 3
}
if (-not (Test-Path -LiteralPath $targetFull -PathType Container)) {
    if (-not $AllowNewTargetDir) {
        Write-Host "拒绝：-TargetDir 不存在（$targetFull）。新目录要从零编译整棵依赖树，实测 D:\Rust\target-l2-verify8 从零编译崩溃 0xc0000409；只用实测可用的目录（默认 D:\Rust\target-l2-m1cwd）。确实要试新目录请加 -AllowNewTargetDir。" -ForegroundColor Red
        exit 3
    }
    Write-Host "  警告：$targetFull 不存在，将从零编译（未经验证，可能很慢或崩溃）。" -ForegroundColor Yellow
}
$targetExe = Join-Path $targetFull 'debug\aios-database.exe'
if (Test-Path -LiteralPath $targetExe -PathType Leaf) {
    # 探一下这份 exe 有没有被别的进程占住：占住了且本次需要重链接，就会 os error 5。只提醒，不拦（源码没变时 cargo 不重链接）。
    try {
        $probe = [System.IO.File]::Open($targetExe, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $probe.Dispose()
    } catch {
        Write-Host "  警告：$targetExe 正被别的进程占用；若本次需要重链接会 os error 5（届时本脚本退出 3，不算 L2 红）。" -ForegroundColor Yellow
    }
}

# ---------------------------------------------------------------- 2. 既有进程清单（只记录，绝不 kill）
$procLines = [System.Collections.Generic.List[string]]::new()
foreach ($name in @('aios-database', 'surreal')) {
    foreach ($p in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
        $path = ''
        $start = ''
        try { $path = [string]$p.Path } catch { $path = '<无权读取>' }
        try { $start = $p.StartTime.ToString('yyyy-MM-dd HH:mm:ss') } catch { $start = '<无权读取>' }
        $procLines.Add(('  pid={0,-7} {1,-14} start={2,-19} path={3}' -f $p.Id, $p.ProcessName, $start, $path))
    }
}
Write-Host "  跑前既有 aios-database / surreal 进程：$($procLines.Count) 个（只记录，不 kill）"
foreach ($line in $procLines) { Write-Host $line -ForegroundColor DarkGray }

# ---------------------------------------------------------------- 3. 影子根
$shadowReady = Test-Path -LiteralPath (Join-Path $ShadowRoot 'AvevaMarineSample') -PathType Container
if (-not $shadowReady) {
    Write-Host "  影子根不存在或不完整，将先造：$ShadowRoot（New-ShadowProjectRoot.ps1，幂等；Target 须与 D:\AVEVA\Projects\E3D3.1 同卷）" -ForegroundColor Yellow
}

if ($DryRun) {
    Write-Host '  -DryRun：到此为止，不跑、不落盘。' -ForegroundColor Yellow
    exit 0
}

New-Item -ItemType Directory -Force -Path $logDir | Out-Null
if (-not $shadowReady) {
    & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'New-ShadowProjectRoot.ps1') -Target $ShadowRoot
    if ($LASTEXITCODE -ne 0) {
        Write-Host "造影子根失败（exit=$LASTEXITCODE）：$ShadowRoot" -ForegroundColor Red
        exit 3
    }
}

# ---------------------------------------------------------------- 4. 跑
$envKeys = @('CARGO_TARGET_DIR', 'AIOS_REPLAY_PROJECT_ROOT', 'AIOS_REPLAY_CASE', 'AIOS_REPLAY_INCLUDE_KNOWN_RED')
$savedEnv = @{}
foreach ($k in $envKeys) { $savedEnv[$k] = [Environment]::GetEnvironmentVariable($k, 'Process') }
$prevOutputEncoding = [Console]::OutputEncoding

$header = [System.Collections.Generic.List[string]]::new()
$header.Add("# L2 replay  stamp=$stamp  owner=$ownerSafe")
$header.Add("# repo=$repo")
$header.Add("# CARGO_TARGET_DIR=$targetFull")
$header.Add("# AIOS_REPLAY_PROJECT_ROOT=$ShadowRoot")
$header.Add("# AIOS_REPLAY_CASE=$caseText")
$header.Add("# AIOS_REPLAY_INCLUDE_KNOWN_RED=$knownRedText")
$header.Add("# features=$Features")
$header.Add("# cmd=cargo $($cargoArgs -join ' ')")
$header.Add("# 跑前既有进程（只记录，不 kill）：$($procLines.Count) 个")
foreach ($line in $procLines) { $header.Add("#$line") }
$header.Add('# ---- cargo 输出 ----')

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$cargoExit = 1
$writer = [System.IO.StreamWriter]::new($logPath, $false, [System.Text.UTF8Encoding]::new($false))
$writer.AutoFlush = $true
try {
    foreach ($line in $header) { $writer.WriteLine($line) }
    # cargo / 被测服务都按 UTF-8 吐中文；按本机 OEM 代码页解码会把日志写成乱码。
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $env:CARGO_TARGET_DIR = $targetFull
    $env:AIOS_REPLAY_PROJECT_ROOT = $ShadowRoot
    if ($Case) { $env:AIOS_REPLAY_CASE = $Case } else { Remove-Item Env:\AIOS_REPLAY_CASE -ErrorAction SilentlyContinue }
    if ($IncludeKnownRed) { $env:AIOS_REPLAY_INCLUDE_KNOWN_RED = '1' } else { Remove-Item Env:\AIOS_REPLAY_INCLUDE_KNOWN_RED -ErrorAction SilentlyContinue }

    & cargo @cargoArgs 2>&1 | ForEach-Object {
        $outLine = "$_"
        Write-Host $outLine
        $writer.WriteLine($outLine)
    }
    $cargoExit = if ($null -ne $LASTEXITCODE) { $LASTEXITCODE } else { 0 }
} catch {
    $outLine = "EXCEPTION: $_"
    Write-Host $outLine -ForegroundColor Red
    $writer.WriteLine($outLine)
    $cargoExit = 1
} finally {
    $sw.Stop()
    $writer.Dispose()
    [Console]::OutputEncoding = $prevOutputEncoding
    foreach ($k in $envKeys) {
        if ($null -eq $savedEnv[$k]) { Remove-Item "Env:\$k" -ErrorAction SilentlyContinue } else { Set-Item -Path "Env:\$k" -Value $savedEnv[$k] }
    }
}

# ---------------------------------------------------------------- 5. 判定（按日志，不只看退出码）
$wall = [math]::Round($sw.Elapsed.TotalSeconds, 1)
$logText = [string](Get-Content -LiteralPath $logPath -Raw -Encoding utf8)

function Select-LogLines([string]$Needle, [int]$Max = 3) {
    $pattern = '(?m)^.*' + [regex]::Escape($Needle) + '.*$'
    return @([regex]::Matches($logText, $pattern) | Select-Object -First $Max | ForEach-Object { $_.Value.Trim() })
}

$osError5 = $logText -match 'os error 5'
$instanceLock = $logText -match 'stage=instance-lock'
$skipLines = @(Select-LogLines 'skip increment_replay:')

$findings = [System.Collections.Generic.List[string]]::new()
$verdict = ''
$scriptExit = $cargoExit
if ($osError5) {
    $findings.Add("【os error 5】target 被占用：别的 aios-database.exe 独占了 debug\aios-database.exe，cargo 重链接失败。这是环境错误，不是 L2 红；cargo 退出码 $cargoExit 不能作判据。换独立 target（默认 D:\Rust\target-l2-m1cwd）或等占用进程退出后重跑。")
    foreach ($l in (Select-LogLines 'os error 5')) { $findings.Add("    $l") }
    $verdict = 'env-os-error-5'
    $scriptExit = 3
} elseif ($instanceLock) {
    $findings.Add('【stage=instance-lock】被测服务抢到了仓根单实例锁（服务按进程 CWD 取 .gen-model.instance.lock，仓根那把常被常驻服务持有）。M1 已修，不该再出现——查 harness 给被测服务的 CWD 是否仍是仓根。')
    foreach ($l in (Select-LogLines 'stage=instance-lock')) { $findings.Add("    $l") }
    $verdict = 'env-instance-lock'
    $scriptExit = 4
} else {
    if ($skipLines.Count -gt 0) {
        $findings.Add('【假绿】日志出现 "skip increment_replay:"：harness 前置不齐被整档跳过，没真跑。典型原因是跨 worktree 复用 target，二进制里嵌的 CARGO_MANIFEST_DIR 指向别的检出（仓根 DbOption.toml / bin\surreal.exe 找不到）；也可能是影子根下没有 AvevaMarineSample。')
        foreach ($l in $skipLines) { $findings.Add("    $l") }
        $verdict = 'fake-green-skip'
        $scriptExit = 2
    }
    if ($cargoExit -eq 0 -and $MinWallClockSec -gt 0 -and $wall -lt $MinWallClockSec) {
        $findings.Add("【假绿】整档墙钟 $wall s 低于下限 $MinWallClockSec s（整档实测 1476 s），没真跑。")
        if (-not $verdict) { $verdict = 'fake-green-wallclock' }
        $scriptExit = 2
    }
    if (-not $verdict) {
        if ($cargoExit -eq 0) {
            $verdict = 'green'
            $findings.Add("【真绿】退出码 0，墙钟 $wall s（下限 $MinWallClockSec s），日志无 skip increment_replay:。")
        } else {
            $verdict = 'l2-red'
            $findings.Add("【L2 红】cargo 退出码 $cargoExit（不是 os error 5、不是 instance-lock），看日志里的 panic / 断言：$logPath")
        }
    }
}

Write-Host ''
Write-Host '===== L2 判定 =====' -ForegroundColor Cyan
Write-Host "  cargo 退出码 = $cargoExit    墙钟 = $wall s    下限 = $MinWallClockSec s"
$color = switch ($verdict) { 'green' { 'Green' } 'l2-red' { 'Red' } default { 'Yellow' } }
foreach ($f in $findings) { Write-Host "  $f" -ForegroundColor $color }
Write-Host "  日志：$logPath"
Write-Host "  汇总：$summaryPath"

$summary = @(
    "stamp=$stamp",
    "owner=$ownerSafe",
    "case=$(if ($Case) { $Case } else { '<all>' })",
    "include_known_red=$(if ($IncludeKnownRed) { 1 } else { 0 })",
    "target_dir=$targetFull",
    "shadow_root=$ShadowRoot",
    "features=$Features",
    "cargo_exit=$cargoExit",
    "wall_sec=$wall",
    "min_wall_sec=$MinWallClockSec",
    "verdict=$verdict",
    "script_exit=$scriptExit",
    "preexisting_processes=$($procLines.Count)",
    "log=$logPath",
    '----'
)
Add-Content -LiteralPath $summaryPath -Value $summary -Encoding utf8
exit $scriptExit
