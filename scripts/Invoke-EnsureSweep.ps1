#Requires -Version 7
<#
按根清单并行调用 /api/v1/model/ensure 做全量生成。
- 幂等可续跑：已有结果行（Generated/AlreadyAvailable/NoRenderableGeometry）跳过
- 每根一行 NDJSON 落盘（named mutex 防交错）
#>
[CmdletBinding()]
param(
    [string]$RootsCsv = (Join-Path (Split-Path $PSScriptRoot -Parent) "_7997_roots.csv"),
    [string]$OutFile  = (Join-Path (Split-Path $PSScriptRoot -Parent) "_7997_ensure_sweep.ndjson"),
    [string]$Endpoint = "http://127.0.0.1:8021/api/v1/model/ensure",
    [int]$Throttle    = 4,
    [int]$TimeoutSec  = 420
)

$ErrorActionPreference = "Stop"
$roots = Import-Csv $RootsCsv

$done = @{}
if (Test-Path $OutFile) {
    foreach ($line in Get-Content $OutFile) {
        try {
            $j = $line | ConvertFrom-Json
            if ($j.status -in @('Generated','AlreadyAvailable','NoRenderableGeometry','NoGenerationRoot')) { $done[$j.refno] = $true }
        } catch {}
    }
}
$todo = @($roots | Where-Object { -not $done.ContainsKey($_.refno) })
"总根数 $($roots.Count)，已完成 $($done.Count)，本轮待跑 $($todo.Count)，并行度 $Throttle"
if ($todo.Count -eq 0) { "SWEEP_DONE"; exit 0 }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$counter = [hashtable]::Synchronized(@{ n = 0; fail = 0 })

$todo | ForEach-Object -ThrottleLimit $Throttle -Parallel {
    $root = $_
    $endpoint = $using:Endpoint
    $outFile  = $using:OutFile
    $timeout  = $using:TimeoutSec
    $counter  = $using:counter
    $sw       = $using:sw
    $total    = ($using:todo).Count

    $body = @{ refno = $root.refno } | ConvertTo-Json -Compress
    $attempt = 0
    $rec = $null
    while ($attempt -lt 2 -and $null -eq $rec) {
        $attempt++
        $t0 = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $resp = Invoke-RestMethod -Method Post -Uri $endpoint -ContentType 'application/json' -Body $body -TimeoutSec $timeout
            $rec = [pscustomobject]@{
                refno = $root.refno; kind = $root.kind; noun = $root.noun; subtree = [int]$root.subtree
                status = $resp.status; generation_root = $resp.generation_root
                renderable = $resp.model_instance_count; written = $resp.generated_instance_count
                ms = $t0.ElapsedMilliseconds; attempt = $attempt; error = $null
            }
        } catch {
            $msg = $_.Exception.Message
            $status = $null
            if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
            $detail = $msg
            try {
                if ($_.Exception.Response.Content) {
                    $detail = $_.Exception.Response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                }
            } catch {}
            if ($status -eq 422 -and $detail -eq $msg) {
                try {
                    $detail = (Invoke-WebRequest -Method Post -Uri $endpoint -ContentType 'application/json' `
                        -Body $body -TimeoutSec $timeout -SkipHttpErrorCheck).Content
                } catch {}
            }
            if ($status -eq 422 -and $detail -match '无法解析生成根|找不到任何合法生成根') {
                $rec = [pscustomobject]@{
                    refno = $root.refno; kind = $root.kind; noun = $root.noun; subtree = [int]$root.subtree
                    status = "NoGenerationRoot"; generation_root = $null
                    renderable = 0; written = 0
                    ms = $t0.ElapsedMilliseconds; attempt = $attempt; error = $detail
                }
            } elseif ($attempt -ge 2 -or ($status -in 400,404,422)) {
                $rec = [pscustomobject]@{
                    refno = $root.refno; kind = $root.kind; noun = $root.noun; subtree = [int]$root.subtree
                    status = "http_$status"; generation_root = $null
                    renderable = $null; written = $null
                    ms = $t0.ElapsedMilliseconds; attempt = $attempt; error = $detail
                }
            } else {
                Start-Sleep -Seconds 3
            }
        }
    }

    $mutex = [System.Threading.Mutex]::new($false, 'Local\ensure7997log')
    [void]$mutex.WaitOne()
    try { Add-Content -Path $outFile -Value ($rec | ConvertTo-Json -Compress) -Encoding UTF8 }
    finally { $mutex.ReleaseMutex(); $mutex.Dispose() }

    $n = 0
    $mutex2 = [System.Threading.Mutex]::new($false, 'Local\ensure7997cnt')
    [void]$mutex2.WaitOne()
    try {
        $counter.n++
        if ($rec.status -like 'http_*') { $counter.fail++ }
        $n = $counter.n
    } finally { $mutex2.ReleaseMutex(); $mutex2.Dispose() }

    if ($n % 25 -eq 0 -or $n -eq $total) {
        "PROGRESS $n/$total fail=$($counter.fail) elapsed=$([math]::Round($sw.Elapsed.TotalMinutes,1))min"
    }
}

"SWEEP_DONE total=$($counter.n) fail=$($counter.fail) elapsed=$([math]::Round($sw.Elapsed.TotalMinutes,1))min"
if ($counter.fail -gt 0) { exit 1 }
