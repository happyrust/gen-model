#requires -Version 7
<#
.SYNOPSIS
    跑回放层（tests/increment_replay）的用例：排队、整理影子工程根、带好环境变量、收日志、逐用例报结果。不编译。

.DESCRIPTION
    回放层把一条录好的会话链逐步「就地追加」给一台自己起的后端（内存外的一次性库、随机端口、只听本机），
    断言走任务回执、水位、库上行。它不碰 E3D，也不碰线上的数据库和后端。

    本脚本用的是已经编好的测试程序（本机配置 output/increment-suite/local.json 的 replay.test_exe；
    改了 tests/increment_replay/*.rs 之后要重编一次，经编译锁）。它做的事：
      1. 排队：同一条道上同一时间只跑一趟回放（几条用例共用影子工程根里的同一个工作文件）。先来先得。
         道数是本机配置的 replay.lanes（不写是 1）：每条道有自己的锁和自己的影子工程根，
         第 1 道的锁是 output/.replay-session.lock，第 N 道是 output/.replay-session.laneN.lock，
         影子工程根是配置的那一份加后缀 -laneN。可提交内存余量不到 -MinCommitGB 就等。
      2. 整理影子工程根：按这次要跑的用例目录重排「哪些库文件由回放自己写」（New-ShadowProjectRoot.ps1，幂等）。
      3. 跑：AIOS_REPLAY_PROJECT_ROOT、AIOS_REPLAY_CASES_DIR、AIOS_REPLAY_CASE、AIOS_RVM_VERIFY_EXE 都替你设好。
      4. 收：日志落在 <runs_dir>/<Who>/replay-<时间戳>-<Label>/replay.log；逐用例一行结果；
         不过的把断言原文的前几行打出来。每条用例自己的运行目录在 output/increment-replay/ 下，
         跑完把里面的一次性库删掉（-KeepStore 留着）。

    -CasesDir   用例目录。缺省是仓里的 tests/increment_replay/cases；新录的、终稿还在本机的用例放在别处
                （比如工作区自己的 replay 目录），用它指过去。
    -Case       只跑这一条（文件名去掉 .json）。不给就跑目录里全部。
    -IncludeKnownRed  连标了 known_red 的也跑。
    -Backend    被测的后端换成本机配置 replay.backends 里登记的某一版（如 r5、r6）。不给就是和测试一起编出来的本地主线。
                同一条用例在新旧两版上各跑一遍，用来证明它分得出新旧。
                本地主线也不直接从 target/debug 跑：跑着的时候 cargo 换不掉那个文件，谁再编一次都会以 101 退出。
                入口把它按文件时间拷一份到本机配置 workspace 下的 backend/bin/aios-database-main-<月日-时分秒>.exe，
                从副本跑；结果里的 backend_exe 记的就是这一份，旧的副本没人用了就删。

    同一趟不排两遍：同一个人、同一个用例目录、同一条用例（或整个目录）、同一版后端的回放已经在排队或在跑时，
    后来的那一次直接退出（退出码 8），不再占一条道。真要并着再跑一遍加 -AllowDuplicate。

    退出码：0 全过；1 有用例不过；2 参数不对；6 没等到；7 本机配置不对或前置缺失；8 同一趟已经在排队或在跑。

.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteReplay.ps1 -Who T1 -CasesDir <你的 replay 目录> -Case t1-compre-hstu
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Who,
    [string]$CasesDir = '',
    [string]$Case = '',
    [string]$Label = '',
    [switch]$IncludeKnownRed,
    [string]$Backend = '',
    [switch]$KeepStore,
    [switch]$AllowDuplicate,
    [int]$WaitMinutes = 60,
    [double]$MinCommitGB = 14,
    [int]$TimeoutMinutes = 60,
    [string]$LocalConfig = ''
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
function Stop-With([int]$code, [string]$message) { Write-Host $message; exit $code }
if ($Who -notmatch '^[A-Za-z0-9_-]+$') { Stop-With 2 '-Who 只用字母、数字、下划线、短横' }
if (-not $LocalConfig) { $LocalConfig = Join-Path $repo 'output\increment-suite\local.json' }
if (-not (Test-Path -LiteralPath $LocalConfig)) { Stop-With 7 "没有本机配置 $LocalConfig" }
$cfg = Get-Content -LiteralPath $LocalConfig -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $cfg.PSObject.Properties['replay']) { Stop-With 7 '本机配置里没有 replay 一节（测试程序和影子工程根在哪）' }
$testExe = [string]$cfg.replay.test_exe
$shadow = [string]$cfg.replay.shadow_root
foreach ($need in @($testExe)) { if (-not (Test-Path -LiteralPath $need)) { Stop-With 7 "缺少前置：$need（本脚本不编译）" } }
$defaultCases = Join-Path $repo 'tests\increment_replay\cases'
if (-not $CasesDir) { $CasesDir = $defaultCases }
if (-not (Test-Path -LiteralPath $CasesDir)) { Stop-With 2 "用例目录不在：$CasesDir" }
$CasesDir = (Resolve-Path -LiteralPath $CasesDir).Path
$caseFiles = @(Get-ChildItem -LiteralPath $CasesDir -Filter '*.json' -File)
if (-not $caseFiles.Count) { Stop-With 2 "用例目录里没有 json：$CasesDir" }
if ($Case -and -not (Test-Path -LiteralPath (Join-Path $CasesDir "$Case.json"))) { Stop-With 2 "没有用例 $Case（在 $CasesDir 里找 $Case.json）" }
# 这一趟要跑的用例落在哪个工程里：决定用哪一份影子工程根，以及哪些库文件由回放自己写
$toRun = if ($Case) { @(Get-Item -LiteralPath (Join-Path $CasesDir "$Case.json")) } else { $caseFiles }
$projects = @(); $replayed = @()
foreach ($file in $toRun) {
    try { $spec = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable } catch { Stop-With 2 "用例 $($file.Name) 不是合法的 JSON：$($_.Exception.Message)" }
    $projects += $(if ($spec.Contains('project') -and $spec['project']) { [string]$spec['project']['name'] } else { 'AvevaMarineSample' })
    $replayed += ([string]$spec['db_file'] -replace '/', '\')
    foreach ($companion in @($spec['companions'])) { if ($companion) { $replayed += ([string]$companion['db_file'] -replace '/', '\') } }
}
$projects = @($projects | Select-Object -Unique); $replayed = @($replayed | Select-Object -Unique)
if ($projects.Count -gt 1) { Stop-With 2 "这一趟的用例落在不止一个工程里（$($projects -join '、')）：一趟只能一个工程，用 -Case 一条一条跑，或者分目录放" }
$mainProject = $projects[0]
$roots = if ($cfg.replay.PSObject.Properties['shadow_roots']) { $cfg.replay.shadow_roots } else { $null }
if ($roots -and $roots.PSObject.Properties[$mainProject]) { $shadow = [string]$roots.PSObject.Properties[$mainProject].Value.root; $dbDir = [string]$roots.PSObject.Properties[$mainProject].Value.db_dir }
elseif ($mainProject -eq 'AvevaMarineSample') { $dbDir = 'ams000' }
else { Stop-With 7 "本机配置的 replay.shadow_roots 里没有工程 $mainProject 的影子工程根" }
# 被测的后端：不给就是和测试一起编的本地主线；-Backend <名字> 用本机配置 replay.backends 里登记的那一版（发布程序的副本）
$backendExe = ''
if ($Backend) {
    $known = if ($cfg.replay.PSObject.Properties['backends']) { $cfg.replay.backends } else { $null }
    if (-not $known -or -not $known.PSObject.Properties[$Backend]) { Stop-With 2 "本机配置的 replay.backends 里没有 '$Backend'（有：$(if ($known) { @($known.PSObject.Properties.Name) -join '、' } else { '无' })）" }
    $backendExe = [string]$known.PSObject.Properties[$Backend].Value
    if (-not (Test-Path -LiteralPath $backendExe)) { Stop-With 7 "后端程序不在：$backendExe" }
}
if (-not $Label) { $Label = if ($Case) { $Case } else { 'all' } }
if ($Backend -and $Label -notmatch "(^|-)$([regex]::Escape($Backend))$") { $Label = "$Label-$Backend" }
$Label = $Label -replace '[^A-Za-z0-9_-]', '-'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$outDir = Join-Path ([string]$cfg.runs_dir) "$Who\replay-$stamp-$Label"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$log = Join-Path $outDir 'replay.log'

# ── 排队拿锁 ──
# 几条道：每条道一把锁、一份影子工程根。队只有一条（先来先得），排到队头的人拿第一条空着的道。
$lanes = if ($cfg.replay.PSObject.Properties['lanes'] -and [int]$cfg.replay.lanes -ge 1) { [int]$cfg.replay.lanes } else { 1 }
function Get-LaneLock([int]$n) { if ($n -le 1) { Join-Path $repo 'output\.replay-session.lock' } else { Join-Path $repo "output\.replay-session.lane$n.lock" } }
$lane = 0
$lock = ''
$queueDir = Join-Path $repo 'output\.replay-session.queue'
[void](New-Item -ItemType Directory -Force -Path $queueDir)
$ticket = Join-Path $queueDir ('{0:yyyyMMddHHmmssfff}-{1}-{2}.ticket' -f (Get-Date), $Who, $PID)
# 这一趟的指纹：谁、哪个用例目录、哪条用例、哪一版后端。写进票和锁，用来认出「同一趟被提交了两遍」。
$batchKey = (@($Who, $CasesDir, $Case, $Backend, [string][bool]$IncludeKnownRed) -join '|').ToLowerInvariant()
$batch = [BitConverter]::ToString([Security.Cryptography.SHA1]::HashData([Text.Encoding]::UTF8.GetBytes($batchKey))).Replace('-', '').Substring(0, 12).ToLowerInvariant()
# 票里的 lanes=1 表示「我认得多条道」。没有这一格的票是旧版入口排的，它只会用第 1 道。
[IO.File]::WriteAllText($ticket, "who=$Who pid=$PID label=$Label lanes=1 batch=$batch")
if (-not $AllowDuplicate) {
    # 先把自己的票放进队里再看：两遍几乎同时提交时，排在后面的那一遍退出
    $dupes = @()
    $mineName = Split-Path -Leaf $ticket
    foreach ($t in (Get-ChildItem -LiteralPath $queueDir -Filter '*.ticket' -File | Sort-Object Name)) {
        if ($t.Name -eq $mineName) { break }
        $m = [regex]::Match($t.Name, '-(\d+)\.ticket$')
        if (-not $m.Success -or -not (Get-Process -Id ([int]$m.Groups[1].Value) -ErrorAction SilentlyContinue)) { continue }
        $text = try { [IO.File]::ReadAllText($t.FullName) } catch { '' }
        if ($text -match "batch=$batch\b") { $dupes += "在排队（进程 $($m.Groups[1].Value)，$($t.LastWriteTime.ToString('HH:mm:ss')) 提交）" }
    }
    for ($n = 1; $n -le $lanes; $n++) {
        $laneLock = Get-LaneLock $n
        if (-not (Test-Path -LiteralPath $laneLock)) { continue }
        $text = try { [IO.File]::ReadAllText($laneLock) } catch { '' }
        $owner = [regex]::Match($text, 'pid:\s*(\d+)')
        if ($owner.Success -and (Get-Process -Id ([int]$owner.Groups[1].Value) -ErrorAction SilentlyContinue) -and $text -match "batch:\s*$batch\b") { $dupes += "在第 $n 道上跑（进程 $($owner.Groups[1].Value)）" }
    }
    if ($dupes.Count) {
        Remove-Item -LiteralPath $ticket -Force -ErrorAction SilentlyContinue
        if (-not @(Get-ChildItem -LiteralPath $outDir -Force).Count) { Remove-Item -LiteralPath $outDir -Force -ErrorAction SilentlyContinue }
        Stop-With 8 "同一趟回放已经$($dupes -join '、')：这一次不再排（同一个人、同一个用例目录、同一条用例、同一版后端）。真要并着再跑一遍加 -AllowDuplicate"
    }
}
function Get-CommitFreeGb { $m = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory; return [math]::Round(($m.CommitLimit - $m.CommittedBytes) / 1GB, 1) }
$waitStart = Get-Date
$lastSay = [datetime]::MinValue
try {
    while ($true) {
        $why = @()
        $freeLane = 0; $busy = @()
        for ($n = 1; $n -le $lanes; $n++) {
            $laneLock = Get-LaneLock $n
            if (Test-Path -LiteralPath $laneLock) {
                $text = try { [IO.File]::ReadAllText($laneLock) } catch { '' }
                $owner = [regex]::Match($text, 'pid:\s*(\d+)')
                if ($owner.Success -and -not (Get-Process -Id ([int]$owner.Groups[1].Value) -ErrorAction SilentlyContinue)) {
                    Write-Host "第 $n 道回放锁的主人（进程 $($owner.Groups[1].Value)）已经不在，清掉"
                    Remove-Item -LiteralPath $laneLock -Force -ErrorAction SilentlyContinue
                }
                else { $busy += (($text -split "`n" | Select-Object -First 3 | ForEach-Object { $_.Trim() }) -join '；'); continue }
            }
            if (-not $freeLane) { $freeLane = $n }
        }
        if (-not $freeLane) { $why += '别人在跑回放（' + ($busy -join ' ‖ ') + '）' }
        $free = Get-CommitFreeGb
        if ($free -lt $MinCommitGB) { $why += "可提交余量 $free GB，不到 $MinCommitGB GB" }
        if (-not $why.Count) {
            $mine = Split-Path -Leaf $ticket
            foreach ($t in (Get-ChildItem -LiteralPath $queueDir -Filter '*.ticket' -File | Sort-Object Name)) {
                if ($t.Name -eq $mine) { break }
                $m = [regex]::Match($t.Name, '-(\d+)\.ticket$')
                if ($m.Success -and (Get-Process -Id ([int]$m.Groups[1].Value) -ErrorAction SilentlyContinue)) {
                    # 前面这位是旧版入口排的（只会用第 1 道），而空着的是第 2 道以后的：它用不了，不算挡在我前面
                    $knowsLanes = try { [IO.File]::ReadAllText($t.FullName) -match 'lanes=1' } catch { $true }
                    if ($freeLane -ge 2 -and -not $knowsLanes) { continue }
                    $why += "排在 $($t.Name -replace '^\d+-', '' -replace '-\d+\.ticket$', '') 后面"; break
                }
                Remove-Item -LiteralPath $t.FullName -Force -ErrorAction SilentlyContinue
            }
        }
        if (-not $why.Count) {
            try {
                $fs = [IO.File]::Open((Get-LaneLock $freeLane), [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
                $bytes = [Text.Encoding]::UTF8.GetBytes("who:  $Who（回放 Invoke-SuiteReplay）`nwhen: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')`nwhat: $Label`npid:  $PID`nlane: $freeLane`nbatch: $batch")
                $fs.Write($bytes, 0, $bytes.Length); $fs.Dispose()
                $lane = $freeLane; $lock = Get-LaneLock $lane
                break
            }
            catch [IO.IOException] { $why += '回放锁刚被别人拿走' }
        }
        if (((Get-Date) - $waitStart).TotalMinutes -ge $WaitMinutes) { Remove-Item -LiteralPath $ticket -Force -ErrorAction SilentlyContinue; Stop-With 6 "等了 $WaitMinutes 分钟还轮不到（$($why -join '；')），没跑" }
        if (((Get-Date) - $lastSay).TotalSeconds -ge 120) { Write-Host ("[{0}] 等：{1}" -f (Get-Date -Format 'HH:mm:ss'), ($why -join '；')); $lastSay = Get-Date }
        Start-Sleep -Seconds 5
    }
}
finally { Remove-Item -LiteralPath $ticket -Force -ErrorAction SilentlyContinue }
# 第 2 道起用自己的影子工程根（工作文件不和第 1 道抢）
if ($lane -gt 1) { $shadow = "$shadow-lane$lane" }
if ($lanes -gt 1) { Write-Host ("[{0}] 轮到了：第 {1} 道（共 {2} 道），影子工程根 {3}" -f (Get-Date -Format 'HH:mm:ss'), $lane, $lanes, (Split-Path -Leaf $shadow)) }

$rc = 1
$runStart = Get-Date
try {
    # ── 影子工程根：按这次的用例目录重排 ──
    $shadowLog = Join-Path $outDir 'shadow-root.log'
    # -Replayed 是数组：经 -File 传不成数组，所以用 -Command 拼一个数组字面量（路径里没有引号）
    $replayedLiteral = '@(' + (($replayed | ForEach-Object { "'" + $_ + "'" }) -join ',') + ')'
    $shadowScript = Join-Path $repo 'scripts\replay\New-ShadowProjectRoot.ps1'
    & pwsh -NoProfile -Command "& '$shadowScript' -Target '$shadow' -MainProject '$mainProject' -DbDir '$dbDir' -Replayed $replayedLiteral; exit `$LASTEXITCODE" *> $shadowLog
    if ($LASTEXITCODE -ne 0) { Stop-With 7 "影子工程根没整理成（退出码 $LASTEXITCODE），看 $shadowLog" }

    # ── 缺省的被测后端：本地主线 debug 版的一份副本（见文件头 -Backend 的说明） ──
    if (-not $backendExe) {
        $mainBackend = Join-Path $repo 'target\debug\aios-database.exe'
        $binDir = if ($cfg.PSObject.Properties['workspace'] -and $cfg.workspace) { Join-Path ([string]$cfg.workspace) 'backend\bin' } else { '' }
        if ($binDir -and (Test-Path -LiteralPath $mainBackend)) {
            [void](New-Item -ItemType Directory -Force -Path $binDir)
            $copy = Join-Path $binDir ('aios-database-main-{0}.exe' -f (Get-Item -LiteralPath $mainBackend).LastWriteTime.ToString('MMdd-HHmmss'))
            if (-not (Test-Path -LiteralPath $copy)) {
                # 两条道可能同时来拷：先拷成自己的临时名，再改名；改名时已经有了就用别人拷好的
                $tmp = "$copy.$PID.tmp"
                Copy-Item -LiteralPath $mainBackend -Destination $tmp -Force
                try { Rename-Item -LiteralPath $tmp -NewName (Split-Path -Leaf $copy) -ErrorAction Stop } catch { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
            }
            if (Test-Path -LiteralPath $copy) {
                $backendExe = $copy
                foreach ($old in Get-ChildItem -LiteralPath $binDir -Filter 'aios-database-main-*.exe' | Where-Object { $_.FullName -ine $copy }) { try { Remove-Item -LiteralPath $old.FullName -Force -ErrorAction Stop } catch { } }
            }
        }
    }

    # ── 跑 ──
    $psi = [Diagnostics.ProcessStartInfo]::new($testExe)
    foreach ($a in @('--test-threads=1', '--nocapture', '--exact', 'replay_cases_land_in_sync_and_match_their_expectations')) { $psi.ArgumentList.Add($a) }
    $psi.WorkingDirectory = $repo
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    foreach ($name in @($psi.Environment.Keys | Where-Object { $_ -like 'AIOS_REPLAY_*' })) { [void]$psi.Environment.Remove($name) }
    $psi.Environment['AIOS_REPLAY_PROJECT_ROOT'] = $shadow
    if ($CasesDir -ine (Resolve-Path -LiteralPath $defaultCases).Path) { $psi.Environment['AIOS_REPLAY_CASES_DIR'] = $CasesDir }
    if ($Case) { $psi.Environment['AIOS_REPLAY_CASE'] = $Case }
    if ($IncludeKnownRed) { $psi.Environment['AIOS_REPLAY_INCLUDE_KNOWN_RED'] = '1' }
    if ($cfg.PSObject.Properties['rvm_verify_exe'] -and $cfg.rvm_verify_exe) { $psi.Environment['AIOS_RVM_VERIFY_EXE'] = [string]$cfg.rvm_verify_exe }
    if ($backendExe) { $psi.Environment['AIOS_REPLAY_SERVER_EXE'] = $backendExe }
    $out = [IO.FileStream]::new($log, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
    $timedOut = $false
    try {
        $proc = [Diagnostics.Process]::Start($psi)
        # 测试程序的正文走标准错误（eprintln），两路并进同一份日志
        $copyOut = $proc.StandardOutput.BaseStream.CopyToAsync($out)
        $copyErr = $proc.StandardError.BaseStream.CopyToAsync($out)
        while (-not $proc.HasExited) {
            Start-Sleep -Seconds 2
            if (((Get-Date) - $runStart).TotalMinutes -gt $TimeoutMinutes) { $timedOut = $true; & taskkill.exe /PID $proc.Id /T /F 2>&1 | Out-Null; break }
        }
        $proc.WaitForExit()
        [void][Threading.Tasks.Task]::WaitAll(@($copyOut, $copyErr), 15000)
        $rc = $proc.ExitCode
    }
    finally { $out.Dispose() }
    if ($timedOut) { Write-Host "超过 $TimeoutMinutes 分钟，已收掉" }
}
finally {
    try { if ((Test-Path -LiteralPath $lock) -and ([IO.File]::ReadAllText($lock) -match "pid:\s*$PID\b")) { Remove-Item -LiteralPath $lock -Force } } catch { }
}

# ── 结果 ──
$lines = @(Get-Content -LiteralPath $log -Encoding UTF8)
# 第一条用例的抬头跟在 `test … ... ` 后面，不在行首
$started = @($lines | ForEach-Object { if ($_ -match '== 用例 (\S+?)（') { $Matches[1] } })
$skipped = @($lines | ForEach-Object { if ($_ -match '== 跳过用例 (\S+?)：') { $Matches[1] } })
$panicAt = -1
for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match 'panicked at') { $panicAt = $i; break } }
$failedCase = if ($panicAt -ge 0 -and $started.Count) { $started[-1] } else { '' }
$results = [ordered]@{}
foreach ($name in $started) { $results[$name] = if ($name -eq $failedCase) { 'fail' } else { 'pass' } }
foreach ($name in $skipped) { $results[$name] = 'skipped' }
$expected = if ($Case) { @($Case) } else { @($caseFiles | ForEach-Object BaseName) }
foreach ($name in $expected) { if (-not $results.Contains($name)) { $results[$name] = 'not_run' } }
$runDirs = @($lines | ForEach-Object { if ($_ -match '运行目录 (.+)$') { $Matches[1].Trim() } })
if (-not $KeepStore) { foreach ($d in $runDirs) { $store = Join-Path $d 'store'; if (Test-Path -LiteralPath $store) { Remove-Item -LiteralPath $store -Recurse -Force -ErrorAction SilentlyContinue } } }
$summary = [ordered]@{
    who = $Who; label = $Label; cases_dir = $CasesDir; case = $Case; started = $runStart.ToString('yyyy-MM-dd HH:mm:ss'); seconds = [math]::Round(((Get-Date) - $runStart).TotalSeconds)
    exit = $rc; verdict = $(if ($rc -eq 0) { 'pass' } else { 'fail' }); results = $results; run_dirs = $runDirs; log = $log
    # 这一趟拿什么跑的：被测后端（名字、文件名、文件时间）、回放程序、工程。新旧对照靠这几格分辨是哪一版的结果。
    backend = $(if ($Backend) { $Backend } else { 'local-debug' })
    backend_exe = $(if ($backendExe) { '{0} ({1})' -f (Split-Path -Leaf $backendExe), (Get-Item -LiteralPath $backendExe).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss') } else { '' })
    test_exe = ('{0} ({1})' -f (Split-Path -Leaf $testExe), (Get-Item -LiteralPath $testExe).LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss'))
    project = $mainProject
    lane = $lane
}
[IO.File]::WriteAllText((Join-Path $outDir 'replay-result.json'), ((ConvertTo-Json -InputObject $summary -Depth 6) + "`n"), (New-Object Text.UTF8Encoding($false)))
foreach ($name in $results.Keys) { Write-Host ('  {0,-8} {1}' -f $results[$name], $name) }
# 用例里回放程序不认识的键：那些格没有判，过了也不算判过
foreach ($line in @($lines | Where-Object { $_ -match '== 用例 \S+ 里有回放程序不认识的键' })) { $text = $line.Substring($line.IndexOf('== 用例')); Write-Host ('  注意 ' + $(if ($text.Length -gt 400) { $text.Substring(0, 400) + '…' } else { $text })) }
if ($panicAt -ge 0) {
    Write-Host "不过的断言（$failedCase）："
    $lines[$panicAt..([math]::Min($panicAt + 14, $lines.Count - 1))] | ForEach-Object { Write-Host ('    ' + $(if ($_.Length -gt 300) { $_.Substring(0, 300) + '…' } else { $_ })) }
}
Write-Host ("{0}：{1} 秒；日志 {2}" -f $summary.verdict, $summary.seconds, $log)
exit $(if ($rc -eq 0) { 0 } else { 1 })
