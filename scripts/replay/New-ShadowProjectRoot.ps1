<#
.SYNOPSIS
    给 L2 回放夹具造一个影子工程根（docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md §3.1 第 6 条）。

.DESCRIPTION
    服务只认 project_path 下的整个 MDB 闭包，没有按 dbnum 指定单个文件的覆盖；回放又只改一个 DESI 文件。
    所以：别的项目目录（ZDJ / SCB / AvevaCatalogue / AvevaPlantSample / …）走 junction，主项目里除 ams000 外的
    子目录也走 junction，ams000 里除被回放的文件外每个文件都是到原件的硬链接（服务只读，E3D 不在这条链上，
    硬链接安全）；被回放的文件**不建**，由 `db_session_fixture replay-init` 写出真文件。

    被回放的文件缺省从 tests/increment_replay/cases/*.json 推导（每条用例的 db_file 与 companions[].db_file 的并集），
    显式给 -Replayed 则以给的为准。漏列的后果是 replay-init 把真工程的原件就地改掉（计划 2026-11-03 TH-1），
    所以这里还做三件事：① 被回放的路径必须在 <MainProject>\<DbDir> 下（别处都是 junction，没法隔离）；
    ② 目标里已有的、不是硬链接的非回放文件（上一次用别的 -Replayed 跑出来的独立文件）删掉重链，免得服务读到陈旧库；
    ③ 造完逐个核对：被回放的路径不存在，其它每个文件都确实是硬链接。

    硬链接要求同一卷：Target 必须和 Source 在同一个盘。重复跑是幂等的（已存在的 junction / 链接跳过）。

.EXAMPLE
    scripts/replay/New-ShadowProjectRoot.ps1 -Target D:\work\plant-code\old\_runs\ams-replay\E3D3.1

.EXAMPLE
    scripts/replay/New-ShadowProjectRoot.ps1 -Target D:\work\plant-code\old\_runs\ams-replay\E3D3.1 -Replayed AvevaMarineSample\ams000\ams8000_0001, AvevaMarineSample\ams000\ams7997_0001
#>
[CmdletBinding()]
param(
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1',
    [Parameter(Mandatory)][string]$Target,
    [string]$MainProject = 'AvevaMarineSample',
    [string]$DbDir = 'ams000',
    # 相对 Source 的路径；这些文件不链接，留给 replay-init。缺省从用例 json 推导。
    [string[]]$Replayed = @(),
    # 推导 -Replayed 用的用例目录。
    [string]$CasesDir = (Join-Path $PSScriptRoot '..\..\tests\increment_replay\cases')
)

$ErrorActionPreference = 'Stop'
$Source = (Resolve-Path -LiteralPath $Source).Path
if ((Split-Path -Qualifier $Source) -ne (Split-Path -Qualifier ([System.IO.Path]::GetFullPath($Target)))) {
    throw "Target 必须和 Source 同卷（硬链接）：$Source vs $Target"
}
New-Item -ItemType Directory -Force -Path $Target | Out-Null
$Target = (Resolve-Path -LiteralPath $Target).Path

# 被回放的文件：显式参数优先，否则从用例 json 推导（db_file ∨ companions[].db_file）。
$replayedFrom = 'param'
if (-not $PSBoundParameters.ContainsKey('Replayed') -or $Replayed.Count -eq 0) {
    $replayedFrom = 'cases'
    $CasesDir = (Resolve-Path -LiteralPath $CasesDir).Path
    $Replayed = @(Get-ChildItem -LiteralPath $CasesDir -Filter *.json | ForEach-Object {
        $case = Get-Content -LiteralPath $_.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($case.db_file) { $case.db_file }
        if ($case.companions) { $case.companions | ForEach-Object { $_.db_file } }
    } | Where-Object { $_ } | Sort-Object -Unique)
    if ($Replayed.Count -eq 0) {
        throw "从 $CasesDir 推导不出任何 db_file：用例目录不对，或者显式给 -Replayed"
    }
}
$Replayed = @($Replayed | ForEach-Object { $_ -replace '/', '\' } | Sort-Object -Unique)
$dbPrefix = (Join-Path $MainProject $DbDir) + '\'
foreach ($rel in $Replayed) {
    if (-not $rel.StartsWith($dbPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "被回放的文件 $rel 不在 $dbPrefix 下：影子根里别处都是 junction，写进去就是写真工程。这个库的用例只能跑在物理副本上，或者把它所在的目录也改成逐文件硬链接再说"
    }
}
$replayedFull = $Replayed | ForEach-Object { [System.IO.Path]::GetFullPath((Join-Path $Source $_)) }

function New-JunctionIfMissing([string]$Link, [string]$To) {
    if (Test-Path -LiteralPath $Link) { return 'kept' }
    cmd /c mklink /J "$Link" "$To" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mklink /J 失败：$Link -> $To" }
    return 'junction'
}

# 文件的硬链接数：`fsutil hardlink list` 一行一个路径（不用 Get-Item 的 LinkType：PowerShell 7 上它不报 HardLink）。
function Get-HardLinkCount([string]$Path) {
    $lines = @(& fsutil hardlink list $Path 2>$null | Where-Object { $_ -and $_.Trim() })
    if ($LASTEXITCODE -ne 0) { throw "fsutil hardlink list 失败：$Path" }
    return $lines.Count
}

$summary = [ordered]@{ junctions = 0; hardlinks = 0; relinked = 0; copied = 0; left_for_replay = @() }

# 1. 根下的文件（custom_evars.bat 等）真拷；其他项目目录 junction。
Get-ChildItem -LiteralPath $Source -File | ForEach-Object {
    $dest = Join-Path $Target $_.Name
    if (-not (Test-Path -LiteralPath $dest)) { Copy-Item -LiteralPath $_.FullName -Destination $dest; $summary.copied++ }
}
Get-ChildItem -LiteralPath $Source -Directory | Where-Object { $_.Name -ne $MainProject } | ForEach-Object {
    if ((New-JunctionIfMissing (Join-Path $Target $_.Name) $_.FullName) -eq 'junction') { $summary.junctions++ }
}

# 2. 主项目：真目录；除 DbDir 外的子目录 junction；根下文件真拷。
$mainSrc = Join-Path $Source $MainProject
$mainDst = Join-Path $Target $MainProject
New-Item -ItemType Directory -Force -Path $mainDst | Out-Null
Get-ChildItem -LiteralPath $mainSrc -File | ForEach-Object {
    $dest = Join-Path $mainDst $_.Name
    if (-not (Test-Path -LiteralPath $dest)) { Copy-Item -LiteralPath $_.FullName -Destination $dest; $summary.copied++ }
}
Get-ChildItem -LiteralPath $mainSrc -Directory | Where-Object { $_.Name -ne $DbDir } | ForEach-Object {
    if ((New-JunctionIfMissing (Join-Path $mainDst $_.Name) $_.FullName) -eq 'junction') { $summary.junctions++ }
}

# 3. DbDir：逐文件硬链接，被回放的文件留空；上次留下的独立文件（不是硬链接）删掉重链。
$dbSrc = Join-Path $mainSrc $DbDir
$dbDst = Join-Path $mainDst $DbDir
New-Item -ItemType Directory -Force -Path $dbDst | Out-Null
Get-ChildItem -LiteralPath $dbSrc -File | ForEach-Object {
    $dest = Join-Path $dbDst $_.Name
    if ($replayedFull -contains $_.FullName) {
        $summary.left_for_replay += $dest
        if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest -Force }
        return
    }
    if (Test-Path -LiteralPath $dest) {
        if ((Get-HardLinkCount $dest) -gt 1) { return }
        # 上一次用别的 -Replayed 跑出来的独立文件：陈旧库，删掉换成链接（删的是影子根里的独立文件，原件不受影响）。
        Remove-Item -LiteralPath $dest -Force
        $summary.relinked++
    }
    New-Item -ItemType HardLink -Path $dest -Target $_.FullName | Out-Null
    $summary.hardlinks++
}
Get-ChildItem -LiteralPath $dbSrc -Directory | ForEach-Object {
    if ((New-JunctionIfMissing (Join-Path $dbDst $_.Name) $_.FullName) -eq 'junction') { $summary.junctions++ }
}

# 4. 自检：被回放的路径必须不存在；其它每个文件必须是硬链接（链接数 > 1）。
$leftSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($dest in $summary.left_for_replay) {
    [void]$leftSet.Add($dest)
    if (Test-Path -LiteralPath $dest) { throw "自检失败：被回放的 $dest 仍存在" }
}
$notLinked = @(Get-ChildItem -LiteralPath $dbDst -File |
    Where-Object { -not $leftSet.Contains($_.FullName) -and (Get-HardLinkCount $_.FullName) -le 1 } |
    ForEach-Object { $_.FullName })
if ($notLinked.Count -gt 0) {
    throw "自检失败：这些文件不是到原件的硬链接（服务会读到陈旧库；原件里已没有的文件请手工处置）：$($notLinked -join '; ')"
}

[pscustomobject]@{
    source = $Source
    target = $Target
    junctions = $summary.junctions
    hardlinks = $summary.hardlinks
    relinked = $summary.relinked
    copied = $summary.copied
    replayed_from = $replayedFrom
    replayed = $Replayed
    left_for_replay = $summary.left_for_replay
} | ConvertTo-Json -Compress
