#Requires -Version 7
<#
从服务 stdout 日志里统计 db 文件解析成本。

解析产物按文件缓存（cata_closure 的 SessionCache），所以「解析事件数 > 不同文件数」
就说明缓存在抖动——差额全是白花的时间。本脚本把两者分开报，并给出理论下限。

日志里每次解析固定产出这三行（parse.rs）：
    read file "<path>" finished in <t>
    gen_ref_type_pos_table: <n> ms
    Parsing children members cost: <n> ms      # 只有全量解析才有；index-only 不产生

用法：
    pwsh -File .\scripts\Measure-ParseCost.ps1 -LogPath _7997_service4.out.log
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$LogPath
)

$ErrorActionPreference = "Stop"

$log = Get-Content -LiteralPath $LogPath
$events = [System.Collections.Generic.List[object]]::new()
$cur = $null
$lineNo = 0

foreach ($line in $log) {
    $lineNo++
    if ($line -match '^read file "(.+?)" finished') {
        if ($cur) { $events.Add([pscustomobject]$cur) }
        $cur = @{
            file = ($matches[1] -split '\\\\')[-1]
            line = $lineNo
            gen  = 0
            memb = 0
        }
    }
    elseif ($cur -and $line -match '^gen_ref_type_pos_table: (\d+) ms') {
        $cur.gen = [long]$matches[1]
    }
    elseif ($cur -and $line -match '^Parsing children members cost: (\d+) ms') {
        $cur.memb = [long]$matches[1]
    }
}
if ($cur) { $events.Add([pscustomobject]$cur) }

if ($events.Count -eq 0) {
    "日志里没有解析事件：$LogPath"
    exit 0
}

$byFile = $events | Group-Object file
$total = ($events | Measure-Object { $_.gen + $_.memb } -Sum).Sum
# 下限＝每个文件只解析一次，且取它最快的那次（同一文件多次解析的差异是噪声）。
$floor = ($byFile | ForEach-Object {
        ($_.Group | ForEach-Object { $_.gen + $_.memb } | Measure-Object -Minimum).Minimum
    } | Measure-Object -Sum).Sum

"== 每个文件 =="
$byFile | ForEach-Object {
    $sum = ($_.Group | Measure-Object { $_.gen + $_.memb } -Sum).Sum
    $min = ($_.Group | ForEach-Object { $_.gen + $_.memb } | Measure-Object -Minimum).Minimum
    [pscustomobject]@{
        file   = $_.Name
        次数   = $_.Count
        合计s  = [math]::Round($sum / 1000, 1)
        最快s  = [math]::Round($min / 1000, 1)
        冗余s  = [math]::Round(($sum - $min) / 1000, 1)
    }
} | Sort-Object 冗余s, 合计s -Descending | Format-Table | Out-String -Width 200

$membCount = @($events | Where-Object { $_.memb -gt 0 }).Count
"== 合计 =="
"解析事件 $($events.Count) 次，涉及 $($byFile.Count) 个文件"
"其中展开了成员树的（非 index-only）: $membCount 次"
"实际总耗时 : $([math]::Round($total / 1000, 1)) s"
"理论下限   : $([math]::Round($floor / 1000, 1)) s"
if ($total -gt 0) {
    "纯冗余     : $([math]::Round(($total - $floor) / 1000, 1)) s（$('{0:P0}' -f (1 - $floor / $total))）"
}

# 解析事件散布在整个日志里＝请求处理中途还在重解析；集中在开头＝只有启动预加载。
$startupCutoff = 2000
$late = @($events | Where-Object { $_.line -gt $startupCutoff })
"启动之后（日志第 $startupCutoff 行以后）仍发生的解析: $($late.Count) 次"
if ($late.Count -gt 0) {
    ($late | ForEach-Object { "$($_.file)@$($_.line)" }) -join '  '
}
