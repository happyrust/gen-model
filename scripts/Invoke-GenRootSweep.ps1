#Requires -Version 7
<#
按 gen_root 表做全量生成 sweep。

与旧的 Invoke-EnsureSweep.ps1 的区别只有一点：根清单和进度都在库里，不再有 CSV 和 NDJSON。
- 待跑清单来自 fn::gen_roots_todo（大子树优先），天然幂等续跑，不用做集合差
- 每根跑完直接 fn::gen_root_report 落库，各 worker 互不干扰，不需要文件锁
- 进度随时可查：RETURN fn::gen_root_progress(7997)
#>
[CmdletBinding()]
param(
    [int]$Dbnum      = 7997,
    [string]$Endpoint = "http://127.0.0.1:8021/api/v1/model/ensure",
    [int]$Throttle   = 8,
    [int]$TimeoutSec = 420,
    [int]$Limit      = 0,
    # 全量重跑：忽略终态，取该 dbnum 的全部根。配 -Force 用于性能复测/基线重建。
    [switch]$All,
    # 服务端 force=true：已生成过的根也整根重来，否则 settled_status 会直接短路返回。
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$invoke = Join-Path $PSScriptRoot "Invoke-Surreal8009.ps1"

$listSql = if ($All) {
    "RETURN (select pe, subtree from gen_root where dbnum = $Dbnum order by subtree desc).pe;"
} else {
    "RETURN fn::gen_roots_todo($Dbnum);"
}
$todo = ((& $invoke -Sql $listSql | ConvertFrom-Json)[0].result)
if ($null -eq $todo) { $todo = @() }
$todo = @($todo)
if ($Limit -gt 0 -and $todo.Count -gt $Limit) { $todo = $todo[0..($Limit - 1)] }

$progress = (& $invoke -Sql "RETURN fn::gen_root_progress($Dbnum);" | ConvertFrom-Json)[0].result
"总根数 $($progress.total)，已完成 $($progress.done)，本轮待跑 $($todo.Count)，并行度 $Throttle，全量=$($All.IsPresent)，force=$($Force.IsPresent)"
if ($todo.Count -eq 0) { "SWEEP_DONE"; exit 0 }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$counter = [hashtable]::Synchronized(@{ n = 0; fail = 0 })

$todo | ForEach-Object -ThrottleLimit $Throttle -Parallel {
    $peId    = $_
    $invoke  = $using:invoke
    $endpoint = $using:Endpoint
    $timeout = $using:TimeoutSec
    $counter = $using:counter
    $sw      = $using:sw
    $total   = ($using:todo).Count
    $force   = ($using:Force).IsPresent

    $rid   = $peId -replace '^pe:', ''
    $refno = $rid.Replace('_', '/')
    $body  = if ($force) {
        @{ refno = $refno; force = $true } | ConvertTo-Json -Compress
    } else {
        @{ refno = $refno } | ConvertTo-Json -Compress
    }

    $attempt = 0
    $rec = $null
    while ($attempt -lt 2 -and $null -eq $rec) {
        $attempt++
        $t0 = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $resp = Invoke-RestMethod -Method Post -Uri $endpoint -ContentType 'application/json' `
                -Body $body -TimeoutSec $timeout
            $rec = @{
                status = $resp.status; renderable = $resp.model_instance_count
                written = $resp.generated_instance_count; ms = $t0.ElapsedMilliseconds
                attempt = $attempt; error = $null
            }
        } catch {
            $code = if ($_.Exception.Response) { [int]$_.Exception.Response.StatusCode } else { 0 }
            # 真正的原因在响应体的 message 里。只记状态行的话，「生成了但一个实例都没落下」
            # 和真正的内部错误长得一模一样，事后没法分类。
            $msg = $_.Exception.Message
            $detail = $_.ErrorDetails.Message
            if ($detail) {
                $parsed = $null
                try { $parsed = $detail | ConvertFrom-Json } catch { }
                $msg = if ($parsed.message) { $parsed.message } else { $detail }
            }
            if ($attempt -ge 2 -or $code -in 400, 404, 422) {
                $rec = @{
                    status = "http_$code"; renderable = $null; written = $null
                    ms = $t0.ElapsedMilliseconds; attempt = $attempt; error = $msg
                }
            } else {
                Start-Sleep -Seconds 3
            }
        }
    }

    $payload = ([pscustomobject]$rec) | ConvertTo-Json -Depth 3 -Compress
    & $invoke -Sql "RETURN fn::gen_root_report($peId, $payload);" | Out-Null

    $n = 0
    $mutex = [System.Threading.Mutex]::new($false, 'Local\genrootsweepcnt')
    [void]$mutex.WaitOne()
    try {
        $counter.n++
        if ($rec.status -like 'http_*') { $counter.fail++ }
        $n = $counter.n
    } finally { $mutex.ReleaseMutex(); $mutex.Dispose() }

    if ($n % 25 -eq 0 -or $n -eq $total) {
        "PROGRESS $n/$total fail=$($counter.fail) elapsed=$([math]::Round($sw.Elapsed.TotalMinutes,1))min"
    }
}

"SWEEP_DONE total=$($counter.n) fail=$($counter.fail) elapsed=$([math]::Round($sw.Elapsed.TotalMinutes,1))min"
