#requires -Version 7
<#
.SYNOPSIS
    从跑宏入口一批的 result.json 生成一份回放用例的骨架：起点会话、每一步推到哪个会话、名字 → refno、
    E3D 导出的快照都替你摆好；「该重建哪些、不该动哪些」这些期望留着你填。

.DESCRIPTION
    一批（Invoke-SuiteMacro.ps1 的一次调用）里各步前后的会话号都记在 result.json 里。本脚本把它翻成
    tests/increment_replay 认的用例 JSON：

      * start = 这一批里第一个写步之前，目标库的会话号；init 在这里做一次全量。
      * 写步前进 2 个会话：N+1（存盘，改动在这里）一步、N+2（QUIT 的尾会话，只记账）一步；只前进 1 个的
        （同值写回）就一步。导出步、只读步让目标库前进了的（登录记账），也各占一步。
      * 导出步出的快照（NN-export.snapshot.json 和 RVM）拷到 output/increment-suite/truth/<用例名>/ 下，
        挂在「库里正是那个状态」的那一步的 rvm_compare 上。
      * -Units 点名的元素（按名字）从各步日志回读的 refno 里查出来，填进 init 的 records_present / generate_prime；
        同名元素在这一批里换过 refno（删后重建）的，两个都列在 _refs 里。

    生成的文件里带下划线的键（_doc、_leg、_refs）回放程序不读，是给人看的。期望要自己填：
    unit_kinds、unit_count、units_absent、plan_totals、geometry_counts、datacenter_deletes……见 tests/increment_replay/case.rs。

    -Final 是这条链的终稿：库文件在这一批之后任何时刻拷下来的一份（相对仓根的路径），由管副本的人给。

.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/New-SuiteReplayCase.ps1 -Batch T1/20261112-041500-x -Name t1-x `
        -Final output/increment-suite/finals/ams/ams8000_0001.sesno1300 -Units /AIOS-SUITE-T1-EQ-A,/AIOS-SUITE-T1-EQ-B -Out <你的 replay 目录>\t1-x.json
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Batch,
    [Parameter(Mandatory)][string]$Name,
    [Parameter(Mandatory)][string]$Final,
    [Parameter(Mandatory)][string]$Out,
    [string[]]$Units = @(),
    [int]$Dbnum = 0,
    [string[]]$Registry = @(),
    [string]$LocalConfig = ''
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
if (-not $LocalConfig) { $LocalConfig = Join-Path $repo 'output\increment-suite\local.json' }
$cfg = Get-Content -LiteralPath $LocalConfig -Raw -Encoding UTF8 | ConvertFrom-Json
$Units = @($Units | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$Registry = @($Registry | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($Name -notmatch '^[a-z0-9][a-z0-9-]*$') { throw '用例名只用小写字母、数字、短横' }
$batchDir = if ([IO.Path]::IsPathRooted($Batch)) { $Batch } else { Join-Path ([string]$cfg.runs_dir) $Batch }
$resultPath = Join-Path $batchDir 'result.json'
if (-not (Test-Path -LiteralPath $resultPath)) { throw "这一批没有 result.json：$batchDir" }
$result = Get-Content -LiteralPath $resultPath -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable
if ($result['verdict'] -ne 'pass') { throw "这一批不是全过的（verdict=$($result['verdict'])），不拿它录链" }
$finalPath = if ([IO.Path]::IsPathRooted($Final)) { $Final } else { Join-Path $repo $Final }
if (-not (Test-Path -LiteralPath $finalPath)) { throw "终稿不在：$finalPath" }
$copyName = [string]$result['copy']
$copyCfg = $cfg.copies.PSObject.Properties[$copyName].Value
$steps = @($result['steps'])
$firstWrite = $steps | Where-Object { $_['kind'] -eq 'write' } | Select-Object -First 1
if (-not $firstWrite) { throw '这一批里没有写步' }
$db = if ($Dbnum) { "$Dbnum" } else { [string]@($firstWrite['dbs'])[0] }
$dbFileRel = [string]$copyCfg.dbs.PSObject.Properties[$db].Value
$marker = Get-Content -LiteralPath (Join-Path ([string]$copyCfg.root) '.private-copy.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$dbFile = ('{0}/{1}' -f $marker.project_name, $dbFileRel) -replace '\\', '/'
$start = [int]$firstWrite['sesno_before'][$db]

$truthDir = Join-Path $repo "output\increment-suite\truth\$Name"
$refs = [ordered]@{}
$outSteps = [System.Collections.Generic.List[object]]::new()
$initCompare = $null
foreach ($s in $steps) {
    foreach ($k in @($s['refs'].Keys)) {
        $v = [string]$s['refs'][$k]
        if (-not $refs.Contains($k)) { $refs[$k] = $v } elseif (@($refs[$k] -split ' → ')[-1] -ne $v) { $refs[$k] = "$($refs[$k]) → $v" }
    }
    $before = [int]$s['sesno_before'][$db]; $after = [int]$s['sesno_after'][$db]
    $compare = $null
    if ($s['kind'] -eq 'export' -and $s['snapshot']) {
        New-Item -ItemType Directory -Force -Path $truthDir | Out-Null
        $snapSrc = [string]$s['snapshot']['file']
        # 证据目录里的文件名本来就带步号（04-export.snapshot.json），照搬
        $leaf = Split-Path -Leaf $snapSrc
        Copy-Item -LiteralPath $snapSrc -Destination (Join-Path $truthDir $leaf) -Force
        $rvmLeaf = Split-Path -Leaf ([string]$s['rvm'])
        Copy-Item -LiteralPath ([string]$s['rvm']) -Destination (Join-Path $truthDir $rvmLeaf) -Force
        $compare = [ordered]@{ snapshot = "output/increment-suite/truth/$Name/$leaf"; rvm = "output/increment-suite/truth/$Name/$rvmLeaf" }
    }
    if ($after -le $start) { if ($compare) { $initCompare = $compare }; continue }   # 起点之前（含正好停在起点上的导出）
    $spec = [string]$s['spec']
    if ($s['kind'] -eq 'write') {
        if ($after -eq $before + 2) {
            $outSteps.Add([ordered]@{ to = $before + 1; _leg = "$spec 的存盘会话（改动在这里）"; task_state = 'succeeded' })
            $outSteps.Add([ordered]@{ to = $before + 2; _leg = "$spec 的尾会话（QUIT 时写的，只记账）"; unit_count = 0 })
        }
        elseif ($after -eq $before + 1) { $outSteps.Add([ordered]@{ to = $after; _leg = "$spec（只前进了一个会话：没有实际改动，或没有尾会话）"; task_state = 'succeeded' }) }
        else { $outSteps.Add([ordered]@{ to = $after; _leg = "$spec（前进了 $($after - $before) 个会话，这里并成一步）"; task_state = 'succeeded' }) }
    }
    elseif ($after -gt $before) {
        $step = [ordered]@{ to = $after; _leg = "$spec（登录记账，$($after - $before) 个会话）"; unit_count = 0 }
        if ($compare) { $step['rvm_compare'] = $compare; $compare = $null }
        $outSteps.Add($step)
    }
    if ($compare) {
        # 导出没有让会话前进：快照挂在上一步上（库里正是那个状态）
        if ($outSteps.Count) { $outSteps[$outSteps.Count - 1]['rvm_compare'] = $compare } else { $initCompare = $compare }
    }
}
$unitRefs = @()
foreach ($u in $Units) {
    if (-not $refs.Contains($u)) { throw "这一批的日志里没有回读到 $u 的 refno（宏里要有相邻的 Q REF / Q NAME）。回读到的名字：$(@($refs.Keys) -join '、')" }
    $unitRefs += @($refs[$u] -split ' → ')[0]
}
$init = [ordered]@{ to = $start }
if ($unitRefs.Count) { $init['records_present'] = @($unitRefs); $init['generate_prime'] = @($unitRefs) }
if ($initCompare) { $init['rvm_compare'] = $initCompare }
$case = [ordered]@{
    name        = $Name
    registry    = @($Registry)
    _doc        = "骨架由 New-SuiteReplayCase.ps1 生成：$($result['who']) 的 $($result['label']) 那一批（$($result['started'])）。期望要自己填。"
    _refs       = $refs
    dbnum       = [int]$db
    db_file     = $dbFile
    source_file = ($Final -replace '\\', '/')
    start       = $start
    init        = $init
    steps       = @($outSteps.ToArray())
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
[IO.File]::WriteAllText($Out, ((ConvertTo-Json -InputObject $case -Depth 10) + "`n"), (New-Object Text.UTF8Encoding($false)))
Write-Host "骨架 → $Out（库 $db，起点会话 $start，$($outSteps.Count) 步；回读到 $($refs.Count) 个名字）"
