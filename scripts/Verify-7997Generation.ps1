#Requires -Version 7
<#
7997 全量生成后的核对：
A. sweep 结果聚合（状态/类别/噪声分类/失败清单/耗时）
B. 库内按 noun 的实例覆盖（written = inst_relate 行；renderable = 有 aabb）
C. 字典几何 noun（primitive∪geomset∪extrusion）与实际覆盖的交叉（独立于进程内审计）
D. 完整性：inst_geo meshed/bad、queue、水位
输出 JSON 到 _7997_verify.json 便于报告引用。
#>
[CmdletBinding()]
param(
    [string]$Sweep = (Join-Path (Split-Path $PSScriptRoot -Parent) "_7997_ensure_sweep.ndjson"),
    [string]$Flags = (Join-Path (Split-Path $PSScriptRoot -Parent) "noun_flags.json"),
    [string]$OutJson = (Join-Path (Split-Path $PSScriptRoot -Parent) "_7997_verify.json")
)

$ErrorActionPreference = "Stop"
function Sql([string]$q) {
    (& (Join-Path $PSScriptRoot "Invoke-Surreal8009.ps1") -Sql $q | ConvertFrom-Json)
}

$report = [ordered]@{}

# ---- A. sweep 聚合 ----
$latestByRoot = @{}
Get-Content $Sweep | ForEach-Object {
    $rec = $_ | ConvertFrom-Json
    $latestByRoot[$rec.refno] = $rec
}
$recs = @($latestByRoot.Values)
$byStatus = $recs | Group-Object status | Sort-Object Count -Descending |
    ForEach-Object { [ordered]@{ status = $_.Name; count = $_.Count } }
$fails = $recs | Where-Object { $_.status -like 'http_*' }
$emptyUnits = @($fails | Where-Object { $_.error -match '\\u6ca1\\u6709\\u843d\\u4e0b\\u4efb\\u4f55\\u6a21\\u578b\\u5b9e\\u4f8b|没有落下任何模型实例' })
$hardFails  = @($fails | Where-Object { $_ -notin $emptyUnits })
$noGenerationRoot = @($recs | Where-Object { $_.status -eq 'NoGenerationRoot' })
$report.sweep = [ordered]@{
    total          = $recs.Count
    by_status      = $byStatus
    renderable_sum = ($recs | Measure-Object renderable -Sum).Sum
    written_sum    = ($recs | Measure-Object written -Sum).Sum
    empty_unit_500 = $emptyUnits.Count
    no_generation_root = $noGenerationRoot.Count
    hard_fail      = $hardFails.Count
    hard_fail_list = @($hardFails | Select-Object refno, noun, subtree, status, error)
    slowest10      = @($recs | Sort-Object { [long]$_.ms } -Descending | Select-Object -First 10 refno, noun, subtree, @{n='sec';e={[math]::Round($_.ms/1000)}})
    empty_unit_by_noun = @($emptyUnits | Group-Object noun | Sort-Object Count -Descending | ForEach-Object { [ordered]@{ noun=$_.Name; count=$_.Count } })
}

# ---- B. 库内按 noun 覆盖 ----
$aliveRows = (Sql "SELECT noun, count() AS cnt FROM pe WHERE dbnum = 7997 AND deleted != true GROUP BY noun;")[0].result
$writtenRows = (Sql "SELECT in.noun AS noun, count() AS cnt FROM inst_relate WHERE string::starts_with(record::id(id),'24381_') GROUP BY noun;")[0].result
$renderRows  = (Sql "SELECT in.noun AS noun, count() AS cnt FROM inst_relate WHERE string::starts_with(record::id(id),'24381_') AND aabb.d != none GROUP BY noun;")[0].result

$alive = @{}; foreach ($r in $aliveRows) { $alive[$r.noun] = $r.cnt }
$written = @{}; foreach ($r in $writtenRows) { $written[$r.noun] = $r.cnt }
$render = @{}; foreach ($r in $renderRows) { $render[$r.noun] = $r.cnt }

# ---- C. 字典几何 noun 交叉 ----
$flags = Get-Content $Flags -Raw | ConvertFrom-Json
$geomNouns = @{}
foreach ($f in $flags) { if ($f.primitive -or $f.geomset -or $f.extrusion) { $geomNouns[$f.noun_name] = $true } }

$cover = foreach ($noun in ($alive.Keys | Sort-Object)) {
    [pscustomobject]@{
        noun      = $noun
        alive     = $alive[$noun]
        written   = if ($written.ContainsKey($noun)) { $written[$noun] } else { 0 }
        renderable= if ($render.ContainsKey($noun)) { $render[$noun] } else { 0 }
        dict_geom = $geomNouns.ContainsKey($noun)
    }
}
$report.noun_coverage = @($cover | Sort-Object alive -Descending)
$report.dict_geom_zero_written = @($cover | Where-Object { $_.dict_geom -and $_.written -eq 0 } | Sort-Object alive -Descending)
$report.nondict_written = @($cover | Where-Object { (-not $_.dict_geom) -and $_.written -gt 0 })

# ---- D. 完整性 ----
$report.integrity = [ordered]@{
    inst_relate_24381 = (Sql "SELECT count() AS c FROM inst_relate WHERE string::starts_with(record::id(id),'24381_') GROUP ALL;")[0].result[0].c
    inst_relate_24381_aabb = (Sql "SELECT count() AS c FROM inst_relate WHERE string::starts_with(record::id(id),'24381_') AND aabb.d != none GROUP ALL;")[0].result[0].c
    inst_relate_total = (Sql "SELECT count() AS c FROM inst_relate GROUP ALL;")[0].result[0].c
    inst_geo_total    = (Sql "SELECT count() AS c FROM inst_geo GROUP ALL;")[0].result[0].c
    inst_geo_unmeshed = (Sql "SELECT count() AS c FROM inst_geo WHERE meshed != true GROUP ALL;")[0].result[0].c
    inst_geo_bad      = (Sql "SELECT count() AS c FROM inst_geo WHERE bad = true GROUP ALL;")[0].result[0].c
    geo_relate_edges  = (Sql "SELECT count() AS c FROM geo_relate GROUP ALL;")[0].result[0].c
    pending_rows      = (Sql "SELECT dbnum, action, status, count() AS cnt FROM model_update_pending GROUP BY dbnum, action, status;")[0].result
    manual_model_pending = (Sql "SELECT count() AS c FROM manual_model_pending GROUP ALL;")[0].result
    watermark_7997    = (Sql "SELECT applied_sesno, file_latest_sesno FROM dbnum_watermark:7997;")[0].result
}

$report | ConvertTo-Json -Depth 8 | Set-Content -Path $OutJson -Encoding UTF8
"验证完成，结果写入 $OutJson"
"sweep: total=$($report.sweep.total) no_root=$($report.sweep.no_generation_root) empty500=$($report.sweep.empty_unit_500) hard_fail=$($report.sweep.hard_fail)"
"inst_relate(7997)=$($report.integrity.inst_relate_24381) 其中有aabb=$($report.integrity.inst_relate_24381_aabb)"
"dict几何但零写出的noun数=$($report.dict_geom_zero_written.Count)"
if ($fails.Count -gt 0 -or $report.dict_geom_zero_written.Count -gt 0) { exit 1 }
