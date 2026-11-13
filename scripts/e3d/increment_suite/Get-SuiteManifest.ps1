#requires -Version 7
<#
.SYNOPSIS
    把主清单 suite-manifest.json 和 manifest.d/*.json 里的片段合成一张用例表：核对、列出、或写成一份合并后的 JSON。

.DESCRIPTION
    不碰 E3D、不碰库，只读清单和宏文件在不在。

      -Mode check   （缺省）核对：用例号全套唯一、宏文件都在、变更类型是总表里有的、状态与净效果取值对、
                    known_gap 写了说明……有不过的退出码 1。另列提醒（没有对照、没有证据目录等），提醒不影响退出码。
      -Mode list    按来源列出每个用例：状态、副本、库、腿数、变更类型。-Owner / -Status 可以筛。
      -Mode json    写一份合并后的 JSON（缺省 output/increment-suite/merged-manifest.json，不进仓库）：
                    每条用例带来源文件、展开好的腿（宏@库号）、副本、MDB，以及片段里那一条的原文（raw）。

    规矩见 SuiteManifest.ps1 的文件头。要跑某个用例：Invoke-SuiteMacro.ps1 -Case <用例号>。

.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/Get-SuiteManifest.ps1
.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/Get-SuiteManifest.ps1 -Mode list -Owner T2
#>
[CmdletBinding()]
param(
    [ValidateSet('check', 'list', 'json')][string]$Mode = 'check',
    [string]$Owner = '',
    [string]$Status = '',
    [string]$Out = ''
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir 'SuiteManifest.ps1')
$merged = Read-SuiteManifest -SuiteDir $suiteDir
$cases = @($merged.cases)
if ($Owner) { $cases = @($cases | Where-Object { $_.owner -eq $Owner }) }
if ($Status) { $cases = @($cases | Where-Object { $_.status -eq $Status }) }

switch ($Mode) {
    'list' {
        foreach ($group in ($cases | Group-Object source)) {
            Write-Host "== $($group.Name)（$($group.Count) 个）"
            foreach ($c in $group.Group) {
                Write-Host ('   {0,-30} {1,-9} {2,-3} 库 {3,-12} {4} 腿  {5}' -f $c.id, $c.status, $c.copy, ($c.db -join '+'), $c.legs.Count, ($c.change_types -join ','))
            }
        }
        exit 0
    }
    'json' {
        if (-not $Out) { $Out = Join-Path $repo 'output\increment-suite\merged-manifest.json' }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
        $doc = [ordered]@{
            generated = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
            sources   = @($merged.sources)
            problems  = @($merged.problems)
            warnings  = @($merged.warnings)
            cases     = @($cases)
        }
        [IO.File]::WriteAllText($Out, ((ConvertTo-Json -InputObject $doc -Depth 20) + "`n"), (New-Object Text.UTF8Encoding($false)))
        Write-Host "合并清单 $($cases.Count) 个用例 → $Out"
        exit $(if ($merged.problems.Count) { 1 } else { 0 })
    }
    default {
        foreach ($s in $merged.sources) {
            $mine = @($merged.cases | Where-Object { $_.source -eq $s.file })
            $byStatus = ($mine | Group-Object status | Sort-Object Name | ForEach-Object { "$($_.Name) $($_.Count)" }) -join '、'
            Write-Host ('{0,-34} {1,3} 个用例（{2}）' -f $s.file, $s.cases, $byStatus)
        }
        Write-Host "合计 $($merged.cases.Count) 个用例，$($merged.sources.Count) 个来源。"
        if ($merged.problems.Count) { Write-Host "不过的 $($merged.problems.Count) 处："; $merged.problems | ForEach-Object { Write-Host "  × $_" } }
        if ($merged.warnings.Count) { Write-Host "提醒 $($merged.warnings.Count) 处："; $merged.warnings | ForEach-Object { Write-Host "  · $_" } }
        if (-not $merged.problems.Count) { Write-Host '核对过了。' }
        exit $(if ($merged.problems.Count) { 1 } else { 0 })
    }
}
