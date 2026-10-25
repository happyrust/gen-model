<#
.SYNOPSIS
    给 L2 回放夹具造一个影子工程根（docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md §3.1 第 6 条）。

.DESCRIPTION
    服务只认 project_path 下的整个 MDB 闭包，没有按 dbnum 指定单个文件的覆盖；回放又只改一个 DESI 文件。
    所以：别的项目目录（ZDJ / SCB / AvevaCatalogue / AvevaPlantSample / …）走 junction，主项目里除 ams000 外的
    子目录也走 junction，ams000 里除被回放的文件外每个文件都是到原件的硬链接（服务只读，E3D 不在这条链上，
    硬链接安全）；被回放的文件**不建**，由 `db_session_fixture replay-init` 写出真文件。

    硬链接要求同一卷：Target 必须和 Source 在同一个盘。重复跑是幂等的（已存在的 junction / 链接跳过）。

.EXAMPLE
    scripts/replay/New-ShadowProjectRoot.ps1 -Target D:\work\plant-code\old\_runs\ams-replay\E3D3.1 -Replayed AvevaMarineSample\ams000\ams8000_0001
#>
[CmdletBinding()]
param(
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1',
    [Parameter(Mandatory)][string]$Target,
    [string]$MainProject = 'AvevaMarineSample',
    [string]$DbDir = 'ams000',
    # 相对 Source 的路径；这些文件不链接，留给 replay-init。
    [string[]]$Replayed = @('AvevaMarineSample\ams000\ams8000_0001')
)

$ErrorActionPreference = 'Stop'
$Source = (Resolve-Path -LiteralPath $Source).Path
if ((Split-Path -Qualifier $Source) -ne (Split-Path -Qualifier ([System.IO.Path]::GetFullPath($Target)))) {
    throw "Target 必须和 Source 同卷（硬链接）：$Source vs $Target"
}
New-Item -ItemType Directory -Force -Path $Target | Out-Null
$Target = (Resolve-Path -LiteralPath $Target).Path
$replayedFull = $Replayed | ForEach-Object { [System.IO.Path]::GetFullPath((Join-Path $Source $_)) }

function New-JunctionIfMissing([string]$Link, [string]$To) {
    if (Test-Path -LiteralPath $Link) { return 'kept' }
    cmd /c mklink /J "$Link" "$To" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mklink /J 失败：$Link -> $To" }
    return 'junction'
}

$summary = [ordered]@{ junctions = 0; hardlinks = 0; copied = 0; left_for_replay = @() }

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

# 3. DbDir：逐文件硬链接，被回放的文件留空。
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
    if (Test-Path -LiteralPath $dest) { return }
    New-Item -ItemType HardLink -Path $dest -Target $_.FullName | Out-Null
    $summary.hardlinks++
}
Get-ChildItem -LiteralPath $dbSrc -Directory | ForEach-Object {
    if ((New-JunctionIfMissing (Join-Path $dbDst $_.Name) $_.FullName) -eq 'junction') { $summary.junctions++ }
}

[pscustomobject]@{
    source = $Source
    target = $Target
    junctions = $summary.junctions
    hardlinks = $summary.hardlinks
    copied = $summary.copied
    left_for_replay = $summary.left_for_replay
} | ConvertTo-Json -Compress
