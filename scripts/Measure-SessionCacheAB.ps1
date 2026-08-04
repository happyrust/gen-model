<#
.SYNOPSIS
  对固定样本根重放 /api/v1/model/ensure，测量单请求耗时。用于 A/B 对比
  `AIOS_DB_SESSION_CACHE_MB` 开关下的解析产物缓存收益。

.DESCRIPTION
  只打 HTTP、只写自己的结果文件，不碰 gen_root 表——基线由
  _7997_gen_root_baseline_precache.json 保存，重放不得污染它。
  force=true 强制整根重生成，才能和「首次生成」的基线口径可比。
#>
[CmdletBinding()]
param(
    [string]$SampleFile = "_7997_ab_sample.json",
    [Parameter(Mandatory = $true)][string]$Label,
    [int]$Throttle = 8,
    [string]$BaseUri = "http://127.0.0.1:8021",
    [int]$TimeoutSec = 300
)

$ErrorActionPreference = 'Stop'
$sample = Get-Content $SampleFile -Raw | ConvertFrom-Json
Write-Host "样本 $($sample.Count) 个根，并行度 $Throttle，标签 $Label"

$sw = [System.Diagnostics.Stopwatch]::StartNew()

$results = $sample | ForEach-Object -ThrottleLimit $Throttle -Parallel {
    $item = $_
    $refno = $item.rid -replace '_', '/'
    $body = @{ refno = $refno; force = $true } | ConvertTo-Json -Compress
    $t = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 'ok'
    $err = $null
    try {
        $null = Invoke-RestMethod -Method Post -Uri "$using:BaseUri/api/v1/model/ensure" `
            -ContentType 'application/json' -Body $body -TimeoutSec $using:TimeoutSec
    } catch {
        $code = try { [int]$_.Exception.Response.StatusCode } catch { 0 }
        $status = "http_$code"
        $err = try { $_.ErrorDetails.Message } catch { $_.Exception.Message }
    }
    $t.Stop()
    [pscustomobject]@{
        rid          = $item.rid
        noun         = $item.noun
        subtree      = [int]$item.subtree
        baseline_ms  = [long]$item.ms
        ms           = [long]$t.Elapsed.TotalMilliseconds
        status       = $status
        error        = $err
    }
}

$sw.Stop()

$out = "_7997_ab_$Label.json"
$results | ConvertTo-Json -Depth 4 | Set-Content $out -Encoding UTF8

$base = ($results | Measure-Object baseline_ms -Sum).Sum
$now = ($results | Measure-Object ms -Sum).Sum
$fail = @($results | Where-Object { $_.status -ne 'ok' })

Write-Host ""
Write-Host "=== $Label ==="
Write-Host ("墙钟          : {0:N1} s" -f $sw.Elapsed.TotalSeconds)
Write-Host ("请求耗时合计  : {0:N1} s  (基线 {1:N1} s)" -f ($now / 1000), ($base / 1000))
Write-Host ("相对基线      : {0:P1}" -f ($now / [math]::Max(1, $base)))
Write-Host ("失败          : $($fail.Count)")
if ($fail.Count -gt 0) { $fail | Select-Object rid, noun, status, error | Format-Table -AutoSize }
Write-Host "明细已写入 $out"
