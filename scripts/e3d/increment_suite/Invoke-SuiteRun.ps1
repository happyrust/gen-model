#requires -Version 7
<#
.SYNOPSIS
    一条命令把清单里的用例（主清单加 manifest.d 的片段）在 E3D 私有副本上跑一遍，逐用例给出「宏」这一层的结果。

.DESCRIPTION
    它只做编排：把用例按来源分组、排好顺序、切成小批，每一批交给 Invoke-SuiteMacro.ps1 去跑（等锁、看内存、
    开会话、判成败都在那边）。每批单独排一次队，批与批之间别人的会话可以插进来。

    顺序：每个来源先跑它的 setup（-SkipSetup 可以免）；用例按清单里的先后；同一条链只跑一次；
    rvm-truth.json 的 run_last 点名的用例挪到本来源的最后（它们会拆掉别的用例要用的元素）。
    一批里有一步不过：这一批到此为止，那个用例记 fail，这一批里排在它后面的用例回到队列；
    然后重跑一次这个来源的 setup 把基线找回来，再接着跑。setup 自己不过，这个来源剩下的用例都记 not_run。

    结果（宏这一层）：pass = 这个用例的各步都过；fail = 有一步不过；not_run = 没轮到它（或 -CheckOnly）。
    片段里声明的 status（draft / macro_ok / verified / known_gap）原样带在结果里；known_gap 说的是后端的行为，
    宏照样应当跑得通，所以宏这一层不因为它放水。

    产物在 <runs_dir>/<Who>/suite-run-<时间戳>/suite-run.json（runs_dir 取自本机配置，不进仓库），每批的证据目录记在里面。
    退出码：0 = 选中的用例宏这一层全过；1 = 有 fail 或 not_run；2 = 参数不对或清单核对不过。

.EXAMPLE
    # 只看会怎么排，不碰 E3D
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteRun.ps1 -Who T0 -Source suite-manifest.json -PlanOnly
.EXAMPLE
    # 主清单 46 个用例全跑
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteRun.ps1 -Who T0 -Source suite-manifest.json
.EXAMPLE
    # 某位的片段，只跑宏已经跑通过的
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteRun.ps1 -Who T2 -Owner T2 -Status macro_ok,verified,known_gap
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Who,
    [string[]]$Source = @(),
    [string[]]$Owner = @(),
    [string[]]$Case = @(),
    [string[]]$Status = @(),
    [int]$BatchSteps = 12,
    [switch]$SkipSetup,
    [switch]$Teardown,
    [switch]$PlanOnly,
    [switch]$CheckOnly,
    [int]$WaitMinutes = 60,
    [string]$LocalConfig = ''
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir 'SuiteManifest.ps1')
function Split-List([string[]]$items) { @($items | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
$Source = @(Split-List $Source); $Owner = @(Split-List $Owner); $Case = @(Split-List $Case); $Status = @(Split-List $Status)

$merged = Read-SuiteManifest -SuiteDir $suiteDir
if ($merged.problems.Count) { $merged.problems | Select-Object -First 20 | ForEach-Object { Write-Host "清单核对不过：$_" }; exit 2 }
$selected = @($merged.cases | Where-Object {
        $c = $_
        ((-not $Source.Count) -or @($Source | Where-Object { $c.source -like "*$_*" }).Count) -and
        ((-not $Owner.Count) -or ($Owner -contains $c.owner)) -and
        ((-not $Case.Count) -or ($Case -ccontains $c.id)) -and
        ((-not $Status.Count) -or ($Status -contains $c.status))
    })
if (-not $selected.Count) { Write-Host '没有选中任何用例'; exit 2 }

$runLast = @()
$truthPath = Join-Path $suiteDir 'rvm-truth.json'
if (Test-Path -LiteralPath $truthPath) {
    $truth = Read-SuiteJson $truthPath
    if ($truth.Contains('run_last')) { $runLast = @($truth['run_last'] | ForEach-Object { [string]$_ }) }
}

# ── 排计划：来源 → 组（副本 + MDB）→ 单元（一个用例，或共用一条链的几个用例）──
$groups = [System.Collections.Generic.List[object]]::new()
foreach ($bySource in ($selected | Group-Object source)) {
    foreach ($byTarget in ($bySource.Group | Group-Object { "$($_.copy)|$($_.mdb)" })) {
        $ordered = @($byTarget.Group | Where-Object { $runLast -notcontains $_.apply }) + @($byTarget.Group | Where-Object { $runLast -contains $_.apply })
        $units = [System.Collections.Generic.List[object]]::new()
        $chainUnit = @{}
        foreach ($c in $ordered) {
            if ($c.chain) {
                if ($chainUnit.ContainsKey($c.chain)) { $chainUnit[$c.chain].ids += $c.id; continue }
                $unit = [pscustomobject]@{ ids = @($c.id); steps = @($c.legs) }
                $chainUnit[$c.chain] = $unit
                $units.Add($unit)
            }
            else { $units.Add([pscustomobject]@{ ids = @($c.id); steps = @($c.legs) }) }
        }
        $first = $byTarget.Group[0]
        $groups.Add([pscustomobject]@{
                source = $bySource.Name; copy = $first.copy; mdb = $first.mdb; units = $units
                setup = @($byTarget.Group | ForEach-Object { $_.setup } | Select-Object -Unique)
                teardown = @($byTarget.Group | ForEach-Object { $_.teardown } | Select-Object -Unique)
            })
    }
}
$totalSteps = 0
foreach ($g in $groups) {
    $n = ($g.units | ForEach-Object { $_.steps.Count } | Measure-Object -Sum).Sum
    $totalSteps += $n
    Write-Host ('{0,-32} 副本 {1} {2,-6} 用例 {3,3} 个，{4,3} 步；setup {5} 步' -f $g.source, $g.copy, $g.mdb, ($g.units | ForEach-Object { $_.ids.Count } | Measure-Object -Sum).Sum, $n, $g.setup.Count)
}
if ($PlanOnly) {
    foreach ($g in $groups) { foreach ($u in $g.units) { Write-Host ('   {0,-44} {1}' -f ($u.ids -join '+'), ($u.steps -join ' → ')) } }
    Write-Host "只排不跑：共 $totalSteps 步（每步一次 E3D 会话）。"
    exit 0
}

# ── 跑 ──
if (-not $LocalConfig) { $LocalConfig = Join-Path $repo 'output\increment-suite\local.json' }
$cfg = Get-Content -LiteralPath $LocalConfig -Raw -Encoding UTF8 | ConvertFrom-Json
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$outDir = Join-Path ([string]$cfg.runs_dir) "$Who\suite-run-$stamp"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$entry = Join-Path $suiteDir 'Invoke-SuiteMacro.ps1'
$results = [ordered]@{}
foreach ($c in $selected) { $results[$c.id] = [ordered]@{ id = $c.id; source = $c.source; owner = $c.owner; declared = $c.status; macro = 'not_run'; reasons = @(); evidence = ''; steps = @() } }
$batchNo = 0

function Invoke-Batch([string]$label, [string]$copy, [string]$mdb, [string[]]$steps) {
    $entryArgs = @('-NoProfile', '-File', $entry, '-Who', $Who, '-Label', $label, '-Copy', $copy, '-WaitMinutes', "$WaitMinutes", '-Macro', ($steps -join ','))
    if ($mdb) { $entryArgs += @('-Mdb', $mdb) }
    if ($CheckOnly) { $entryArgs += '-CheckOnly' }
    $console = Join-Path $outDir "$label.console.log"
    & pwsh @entryArgs *> $console
    $rc = $LASTEXITCODE
    $dir = Get-ChildItem -LiteralPath (Join-Path ([string]$cfg.runs_dir) $Who) -Directory -Filter "*-$label" | Sort-Object Name | Select-Object -Last 1
    $resultPath = if ($dir) { Join-Path $dir.FullName 'result.json' } else { '' }
    $result = if ($resultPath -and (Test-Path -LiteralPath $resultPath)) { Get-Content -LiteralPath $resultPath -Raw -Encoding UTF8 | ConvertFrom-Json } else { $null }
    return [pscustomobject]@{ rc = $rc; result = $result; dir = $(if ($dir) { $dir.FullName.Substring(([string]$cfg.runs_dir).TrimEnd('\').Length + 1) } else { '' }); console = $console }
}

foreach ($g in $groups) {
    $tag = ([IO.Path]::GetFileNameWithoutExtension($g.source) -replace '[^A-Za-z0-9]', '').ToLower()
    $queue = [System.Collections.Generic.Queue[object]]::new()
    foreach ($u in $g.units) { $queue.Enqueue($u) }
    $needSetup = -not $SkipSetup -and $g.setup.Count -gt 0
    $aborted = $false
    while ($queue.Count -and -not $aborted) {
        if ($needSetup) {
            $batchNo++
            $r = Invoke-Batch ('{0}-{1:d2}-setup' -f $tag, $batchNo) $g.copy $g.mdb $g.setup
            Write-Host ('[{0}] {1} setup：退出码 {2}' -f (Get-Date -Format 'HH:mm:ss'), $g.source, $r.rc)
            if ($r.rc -ne 0) {
                $why = if ($r.result) { @($r.result.steps | Where-Object { $_.verdict -ne 'pass' } | ForEach-Object { $_.reasons } | Select-Object -First 1) -join '' } else { "入口退出码 $($r.rc)，见 $($r.console)" }
                foreach ($u in $queue) { foreach ($id in $u.ids) { $results[$id].reasons = @("setup 没过：$why") } }
                $aborted = $true
                break
            }
            $needSetup = $false
        }
        # 取一批：凑到 BatchSteps 步为止（单元不拆开）
        $batch = [System.Collections.Generic.List[object]]::new()
        $count = 0
        while ($queue.Count -and ($batch.Count -eq 0 -or $count + $queue.Peek().steps.Count -le $BatchSteps)) { $u = $queue.Dequeue(); $batch.Add($u); $count += $u.steps.Count }
        $steps = @($batch | ForEach-Object { $_.steps })
        $batchNo++
        $r = Invoke-Batch ('{0}-{1:d2}' -f $tag, $batchNo) $g.copy $g.mdb $steps
        $stepResults = if ($r.result) { @($r.result.steps) } else { @() }
        $offset = 0
        $failedAt = -1
        for ($i = 0; $i -lt $batch.Count; $i++) {
            $u = $batch[$i]
            $mine = @($stepResults | Where-Object { $_.index -gt $offset -and $_.index -le $offset + $u.steps.Count })
            $offset += $u.steps.Count
            $verdict = if ($CheckOnly) { 'not_run' } elseif ($mine.Count -eq $u.steps.Count -and -not @($mine | Where-Object { $_.verdict -ne 'pass' }).Count) { 'pass' } elseif (@($mine | Where-Object { $_.verdict -ne 'pass' }).Count) { 'fail' } else { 'not_run' }
            if ($verdict -eq 'fail' -and $failedAt -lt 0) { $failedAt = $i }
            if ($verdict -eq 'not_run' -and -not $CheckOnly -and $failedAt -ge 0) { continue }   # 排在失败用例后面的，回队列重跑
            foreach ($id in $u.ids) {
                $results[$id].macro = $verdict
                $results[$id].evidence = $r.dir
                $results[$id].steps = @($mine | ForEach-Object { [ordered]@{ spec = $_.spec; verdict = $_.verdict; seconds = $_.seconds; sesno_before = $_.sesno_before; sesno_after = $_.sesno_after } })
                $results[$id].reasons = @($mine | Where-Object { $_.verdict -ne 'pass' } | ForEach-Object { $_.reasons } | Select-Object -First 3)
                if ($verdict -eq 'not_run' -and -not $CheckOnly) { $results[$id].reasons = @("入口退出码 $($r.rc)，这一批没跑起来，见 $($r.console)") }
            }
        }
        $done = @($batch | ForEach-Object { $_.ids }).Count
        Write-Host ('[{0}] {1} 第 {2} 批：{3} 步，退出码 {4}；{5}' -f (Get-Date -Format 'HH:mm:ss'), $g.source, $batchNo, $steps.Count, $r.rc, $(if ($failedAt -ge 0) { '不过的用例 ' + ($batch[$failedAt].ids -join '+') } else { "$done 个用例" }))
        if ($failedAt -ge 0) {
            # 失败用例后面的回到队首，先重建基线
            $rest = @(); for ($i = $failedAt + 1; $i -lt $batch.Count; $i++) { $rest += $batch[$i] }
            $old = @($queue.ToArray()); $queue.Clear(); foreach ($u in ($rest + $old)) { $queue.Enqueue($u) }
            $needSetup = $g.setup.Count -gt 0
        }
        elseif ($r.rc -ne 0 -and -not $CheckOnly) { $aborted = $true }   # 没有哪一步红却退出码非零：排不上队、配置不对之类，别空转
    }
    if ($Teardown -and $g.teardown.Count -and -not $CheckOnly) {
        $batchNo++
        $r = Invoke-Batch ('{0}-{1:d2}-teardown' -f $tag, $batchNo) $g.copy $g.mdb $g.teardown
        Write-Host ('[{0}] {1} teardown：退出码 {2}' -f (Get-Date -Format 'HH:mm:ss'), $g.source, $r.rc)
    }
}

$all = @($results.Values)
$summary = [ordered]@{
    who = $Who; started = $stamp; finished = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'); check_only = [bool]$CheckOnly
    counts = [ordered]@{ pass = @($all | Where-Object { $_.macro -eq 'pass' }).Count; fail = @($all | Where-Object { $_.macro -eq 'fail' }).Count; not_run = @($all | Where-Object { $_.macro -eq 'not_run' }).Count }
    cases = $all
}
$outFile = Join-Path $outDir 'suite-run.json'
[IO.File]::WriteAllText($outFile, ((ConvertTo-Json -InputObject $summary -Depth 10) + "`n"), (New-Object Text.UTF8Encoding($false)))
Write-Host ("宏这一层：pass {0} / fail {1} / not_run {2}（共 {3} 个用例）。结果 {4}" -f $summary.counts.pass, $summary.counts.fail, $summary.counts.not_run, $all.Count, $outFile)
foreach ($c in ($all | Where-Object { $_.macro -eq 'fail' })) { Write-Host ('  × {0}：{1}' -f $c.id, (@($c.reasons) | Select-Object -First 1)) }
exit $(if ($CheckOnly -or ($summary.counts.fail -eq 0 -and $summary.counts.not_run -eq 0)) { 0 } else { 1 })
