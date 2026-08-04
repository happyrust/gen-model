#Requires -Version 7
<#
把历史 sweep 的 NDJSON 结果导进 gen_root 的进度字段（一次性迁移，可重复运行）。

只写进度字段，根的身份字段由 fn::sync_gen_roots 维护；NDJSON 里已经不是根的 refno 会被
跳过，不会凭空造出 gen_root 行。切换到库驱动的 sweep 之后本脚本就没用了。
#>
[CmdletBinding()]
param(
    [string]$Ndjson  = "d:\work\plant-code\old\gen-model\_7997_ensure_sweep.ndjson",
    [int]$Dbnum      = 7997,
    [int]$ChunkSize  = 500
)

$ErrorActionPreference = "Stop"
$invoke = Join-Path $PSScriptRoot "Invoke-Surreal8009.ps1"

$recs = Get-Content $Ndjson | ForEach-Object { $_ | ConvertFrom-Json }
"NDJSON 记录: $($recs.Count)"

# 同一 refno 可能被重跑过，保留最后一条。
$latest = [ordered]@{}
foreach ($r in $recs) { $latest[$r.refno] = $r }
"去重后: $($latest.Count)"

$known = ((& $invoke -Sql "SELECT VALUE record::id(pe) FROM gen_root WHERE dbnum = $Dbnum;" |
    ConvertFrom-Json)[0].result) | ForEach-Object { $_.Replace('_', '/') }
$knownSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$known)
"gen_root 现有根: $($knownSet.Count)"

$todo = @($latest.Values | Where-Object { $knownSet.Contains($_.refno) })
$skipped = $latest.Count - $todo.Count
"待导入: $($todo.Count)，跳过（已不是根）: $skipped"

$imported = 0
for ($i = 0; $i -lt $todo.Count; $i += $ChunkSize) {
    $chunk = $todo[$i..([Math]::Min($i + $ChunkSize - 1, $todo.Count - 1))]
    $payload = @($chunk | ForEach-Object {
        [pscustomobject]@{
            rid        = $_.refno.Replace('/', '_')
            status     = $_.status
            renderable = $_.renderable
            written    = $_.written
            ms         = $_.ms
            attempt    = $_.attempt
            error      = $_.error
        }
    }) | ConvertTo-Json -Depth 4 -Compress
    if ($chunk.Count -eq 1) { $payload = "[$payload]" }

    $sql = @"
LET `$recs = $payload;
FOR `$r IN `$recs {
    UPDATE type::thing('gen_root', `$r.rid) SET
        status = `$r.status, renderable = `$r.renderable, written = `$r.written,
        ms = `$r.ms, attempt = `$r.attempt, error = `$r.error, updated_at = time::now();
};
"@
    $res = & $invoke -Sql $sql | ConvertFrom-Json
    $bad = @($res | Where-Object { $_.status -ne 'OK' })
    if ($bad.Count -gt 0) { throw "导入失败: $($bad[0].result)" }
    $imported += $chunk.Count
    "  已导入 $imported/$($todo.Count)"
}

$after = (& $invoke -Sql @"
SELECT count() AS c FROM gen_root WHERE dbnum = $Dbnum GROUP ALL;
SELECT status, count() AS cnt FROM gen_root WHERE dbnum = $Dbnum GROUP BY status;
"@ | ConvertFrom-Json)
"gen_root 总行数: $($after[0].result[0].c)"
$after[1].result | ForEach-Object { "  {0,-24} {1}" -f ($_.status ?? '<未跑>'), $_.cnt }
