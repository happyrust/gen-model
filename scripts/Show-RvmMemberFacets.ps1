<#
.SYNOPSIS
    把 `rvm_facets_dump` 倒出来的一个成员逐图元摊开：每只图元的 kind、世界 AABB、局部 AABB。

.DESCRIPTION
    对拍报出某一件超差时，先要知道「E3D 那边到底画了几只、各自摆在哪」。compare.json 只给
    整件的合并 AABB，facets.json 里才有逐图元的面片顶点。本脚本把顶点按图元分组算 AABB，
    并可选地乘 `world⁻¹` 回到设计件的局部系——与 `rvm_compare` 的判据同一口径。

.PARAMETER Facets
    `rvm_facets_dump --out` 产出的 json。

.PARAMETER Name
    成员名（RVM 里的显示名，支持子串匹配），如 `/FL0201`。

.PARAMETER Elements
    可选，`gen_ams` 的 `elements.json`；给了就用里面同 refno 的 `world` 把顶点转回局部系。

.PARAMETER Refno
    配合 `-Elements` 使用，指定拿哪一件的 `world`。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Facets,
    [Parameter(Mandatory)][string]$Name,
    [string]$Elements,
    [string]$Refno
)

$ErrorActionPreference = 'Stop'

$dump = Get-Content $Facets -Raw | ConvertFrom-Json
$member = $dump | Where-Object { $_.name -eq $Name } | Select-Object -First 1
if (-not $member) { $member = $dump | Where-Object { $_.name -like "*$Name*" } | Select-Object -First 1 }
if (-not $member) { throw "facets 里没有名字含 '$Name' 的成员" }

Write-Host ("成员 '{0}'  图元 {1} 只" -f $member.name, @($member.geometries).Count) -ForegroundColor Cyan

# 可选：回到局部系。world 是 4 行 3 列（三个基向量 + 原点），与 elements.json 一致。
$ax = $ay = $az = $o = $null
if ($Elements -and $Refno) {
    $els = Get-Content $Elements -Raw | ConvertFrom-Json
    $el = $els | Where-Object { $_.refno -eq $Refno } | Select-Object -First 1
    if ($el) {
        $ax = $el.world[0]; $ay = $el.world[1]; $az = $el.world[2]; $o = $el.world[3]
        Write-Host ("  （已按 {0} 的 world 逆变换回局部系）" -f $Refno) -ForegroundColor DarkGray
    }
}

$i = 0
foreach ($g in $member.geometries) {
    $i++
    # `polygons_world_mm` 的嵌套深度随图元种类变（多边形 → 轮廓 → 顶点），统一递归摊平，
    # 认「三个数」那一层是顶点。
    $bag = New-Object 'System.Collections.Generic.List[object]'
    $stack = New-Object 'System.Collections.Stack'
    $stack.Push($g.polygons_world_mm)
    while ($stack.Count -gt 0) {
        $node = $stack.Pop()
        if ($node -isnot [System.Array]) { continue }
        if ($node.Count -eq 3 -and ($node[0] -isnot [System.Array])) { $bag.Add($node); continue }
        foreach ($child in $node) { $stack.Push($child) }
    }
    $pts = $bag.ToArray()
    if (-not $pts) { Write-Host ("  [{0}] kind={1}  （没有面片）" -f $i, $g.kind); continue }
    if ($null -ne $o) {
        $local = New-Object 'System.Collections.Generic.List[object]'
        foreach ($p in $pts) {
            $dx = $p[0] - $o[0]; $dy = $p[1] - $o[1]; $dz = $p[2] - $o[2]
            $local.Add(@(
                    $dx * $ax[0] + $dy * $ax[1] + $dz * $ax[2],
                    $dx * $ay[0] + $dy * $ay[1] + $dz * $ay[2],
                    $dx * $az[0] + $dy * $az[1] + $dz * $az[2]
                ))
        }
        $pts = $local.ToArray()
    }
    $mins = @(0, 1, 2) | ForEach-Object { $ax = $_; ($pts | ForEach-Object { $_[$ax] } | Measure-Object -Minimum).Minimum }
    $maxs = @(0, 1, 2) | ForEach-Object { $ax = $_; ($pts | ForEach-Object { $_[$ax] } | Measure-Object -Maximum).Maximum }
    Write-Host ("  [{0}] kind={1,-12} 顶点={2,-5} min=({3,10:N3},{4,10:N3},{5,10:N3})  max=({6,10:N3},{7,10:N3},{8,10:N3})  跨度=({9:N1} x {10:N1} x {11:N1})" -f `
            $i, $g.kind, $pts.Count, $mins[0], $mins[1], $mins[2], $maxs[0], $maxs[1], $maxs[2], ($maxs[0] - $mins[0]), ($maxs[1] - $mins[1]), ($maxs[2] - $mins[2]))
}
