<#
.SYNOPSIS
    SLYK（YCYK-E3D）加载抽样：从直读树走下去，取一批交付单元根挨个 `model/ensure`，
    把「画得出来 / 画不出来 / 报错」三档的数量与样例打出来。

.DESCRIPTION
    只读服务端的按需生成口径（POST /api/v1/model/ensure），不改配置、不写文件。
    用来回答「这个项目的树能不能走通、几何能不能生成出来」，而不是只看服务起没起来。

.PARAMETER Base
    模型服务地址，默认 http://127.0.0.1:8036。

.PARAMETER Sample
    最多 ensure 多少个生成根，默认 30。

.PARAMETER MaxNodes
    树遍历的节点预算，默认 3000（够把候选凑够就停）。
#>
[CmdletBinding()]
param(
    [string]$Base = 'http://127.0.0.1:8036',
    [int]$Sample = 30,
    [int]$MaxNodes = 3000,
    [string[]]$Nouns = @('EQUI', 'BRAN', 'SUPPO', 'HANG')
)

$ErrorActionPreference = 'Stop'
$unitNouns = $Nouns

function Get-Children {
    param([string]$Refno)
    try {
        $r = Invoke-RestMethod "$Base/api/v1/tree/children?refno=$([uri]::EscapeDataString($Refno))" -TimeoutSec 30
        return @($r.nodes)
    }
    catch { return @() }
}

$roots = (Invoke-RestMethod "$Base/api/v1/tree/roots" -TimeoutSec 30).nodes
Write-Host ("根 {0} 个（SITE 平铺）" -f @($roots).Count)

# 广度优先，遇到交付单元类型就收下并不再往里走。
$queue = [System.Collections.Queue]::new()
foreach ($n in $roots) { $queue.Enqueue($n) }
$units = [System.Collections.Generic.List[object]]::new()
$visited = 0
while ($queue.Count -gt 0 -and $units.Count -lt $Sample -and $visited -lt $MaxNodes) {
    $node = $queue.Dequeue()
    $visited++
    if ($unitNouns -contains $node.noun) { $units.Add($node); continue }
    if ($node.children_count -gt 0) {
        foreach ($c in Get-Children $node.refno) { $queue.Enqueue($c) }
    }
}
Write-Host ("遍历 {0} 个节点，取到 {1} 个生成根候选" -f $visited, $units.Count)

$results = [System.Collections.Generic.List[object]]::new()
foreach ($u in $units) {
    $row = [ordered]@{ refno = $u.refno; noun = $u.noun; name = $u.name; dbnum = $u.dbnum }
    try {
        $body = @{ refno = $u.refno } | ConvertTo-Json -Compress
        $res = Invoke-RestMethod "$Base/api/v1/model/ensure" -Method Post -ContentType 'application/json' -Body $body -TimeoutSec 180
        $row.status = $res.status
        $row.model_instance_count = $res.model_instance_count
        $row.generated_instance_count = $res.generated_instance_count
        $row.http = 200
    }
    catch {
        $row.status = 'HTTP_ERROR'
        $row.http = try { [int]$_.Exception.Response.StatusCode } catch { -1 }
        $row.error = $_.ErrorDetails.Message ?? $_.Exception.Message
    }
    $results.Add([pscustomobject]$row)
}

Write-Host ''
Write-Host '=== 按 status 汇总 ===' -ForegroundColor Cyan
$results | Group-Object status | Sort-Object Count -Descending |
    ForEach-Object { Write-Host ("  {0,-24} {1}" -f $_.Name, $_.Count) }

Write-Host ''
Write-Host '=== 逐条 ===' -ForegroundColor Cyan
$results | ForEach-Object {
    Write-Host ("  {0,-14} {1,-6} db={2,-6} {3,-22} inst={4}/{5} {6}" -f
        $_.refno, $_.noun, $_.dbnum, $_.status, $_.model_instance_count, $_.generated_instance_count,
        ($_.name ?? ''))
}

$results | ConvertTo-Json -Depth 5
