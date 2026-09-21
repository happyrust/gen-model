# 初始化 run 的完成守望：轮询 /health、/dbnums、/tasks，判 DONE / FAILED / TIMEOUT。
#
# 取代各 run 目录里那份一次性的 monitor.ps1。那一份的 DONE 判据在 2026-09-10 之后一条都不成立，
# 三处读的都是已经不存在的路径：
#
#   1. `$h.room_build.status`      —— 房间那一路的结论在 `room_build.resident.status`，
#                                     `room_build` 这一层从来没有 `status` 这一格；
#   2. `$h.room_build.rebuild_required` —— 项目级启动全量重建退役之后那枚凭据永远盖不上，
#                                     此字段恒为 true（2026-09-10 已改成只报持久标志本身）；
#   3. `$h.model_publish_queue.*` / `$h.local_runtime.*` —— 两节随发布队列 / backup lane 一起删了。
#
# 另有一处 PowerShell 的坑：原脚本用 `$Dbnums | Where-Object { … [int]$_.dbnum -eq $_ … }`，
# 内层 `$_` 把外层的库号遮住了，那个判据实际恒真。
param(
    [int]$Port = 18422,
    [int[]]$Dbnums = @(1112, 7997),
    [string]$Project = 'AvevaMarineSample',
    [int]$TimeoutMins = 150,
    [int]$PollSecs = 20,
    [string]$RunDir = $PSScriptRoot,
    [switch]$Once
)
$ErrorActionPreference = 'Continue'
$log = Join-Path $RunDir 'monitor.log'
$snapDir = Join-Path $RunDir 'snapshots'
New-Item -ItemType Directory -Force $snapDir | Out-Null

function Log([string]$line) {
    $stamp = Get-Date -Format 'HH:mm:ss'
    "$stamp $line" | Tee-Object -FilePath $log -Append
}

function RowText($r) {
    if (-not $r) { return '(no row)' }
    'applied={0} file={1} model={2} room={3} verdict={4}{5} chasing={6} dead={7}' -f `
        $r.applied_sesno, $r.file_latest_sesno, $r.model_sesno, $r.room_sesno, $r.model_verdict,
    $(if ($r.model_verdict_reason) { "($($r.model_verdict_reason))" } else { '' }),
    $r.model_chasing_roots, $r.model_dead_roots
}

# `Invoke-RestMethod` 把 JSON 变成 PSCustomObject，缺字段读出来是 $null 而不是报错——
# DONE 判据里每一格都得先确认它真的在，否则「读不到」会被当成「不满足」而静默空转。
function Field($object, [string]$path) {
    $cursor = $object
    foreach ($name in $path.Split('.')) {
        if ($null -eq $cursor) { return $null }
        $property = $cursor.PSObject.Properties[$name]
        if ($null -eq $property) { return $null }
        $cursor = $property.Value
    }
    $cursor
}

$deadline = (Get-Date).AddMinutes($TimeoutMins)
$started = Get-Date
Log "monitor start; port=$Port dbnums=$($Dbnums -join ',') timeout=${TimeoutMins}m"

while ((Get-Date) -lt $deadline) {
    try {
        $h = Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/health" -TimeoutSec 120
        $d = Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/dbnums?project=$Project" -TimeoutSec 180
        $t = Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/tasks?limit=100" -TimeoutSec 120
    }
    catch {
        Log "service not answering: $($_.Exception.Message)"
        if ($Once) { exit 4 }
        Start-Sleep -Seconds $PollSecs
        continue
    }

    $stamp = Get-Date -Format 'HHmmss'
    $h | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $snapDir "health-$stamp.json") -Encoding UTF8

    $roomStatus = Field $h 'room_build.resident.status'
    $rebuildRequired = Field $h 'room_build.rebuild_required'
    $spatialState = Field $h 'spatial_tree.state'
    $spatialEntries = Field $h 'spatial_tree.entries'

    $lines = @()
    foreach ($n in $Dbnums) {
        $row = $d.dbnums | Where-Object { [int]$_.dbnum -eq $n } | Select-Object -First 1
        $lines += "dbnum=$n " + (RowText $row)
    }
    $states = @{}
    foreach ($task in $t.tasks) {
        $key = "$($task.kind):$($task.state)"
        if ($states.ContainsKey($key)) { $states[$key]++ } else { $states[$key] = 1 }
    }
    $runText = (($t.tasks | Where-Object { $_.state -eq 'running' } |
            ForEach-Object { "$($_.dbnum)/$($_.kind)@$($_.current_stage)" }) -join ' ')
    $lines += 'tasks: ' + (($states.Keys | Sort-Object | ForEach-Object { "$_=$($states[$_])" }) -join ' ') + " running=[$runText]"
    $lines += 'health: status={0} data_ready={1} model_ready={2} consistency={3} room={4} rebuild_required={5} spatial={6}/{7}' -f `
        $h.status, (Field $h 'initialization.data_ready'), (Field $h 'initialization.model_ready'),
    ((Field $h 'dbnum_consistency' | Select-Object judged, in_sync, lagging, chasing_roots, dead_roots | ConvertTo-Json -Compress)),
    $roomStatus, $rebuildRequired, $spatialState, $spatialEntries
    Log ($lines -join "`n    ")

    $dataTasks = @($t.tasks | Where-Object { $_.kind -eq 'data_batch' })
    $terminal = @($dataTasks | Where-Object { $_.state -in 'succeeded', 'partial', 'failed' })
    # 每个点名的库都要：水位建立、追平文件、三水位齐、判词 in_sync。
    $rowsDone = $true
    foreach ($n in $Dbnums) {
        $row = $d.dbnums | Where-Object { [int]$_.dbnum -eq $n } | Select-Object -First 1
        if (-not $row -or
            [int]$row.applied_sesno -le 0 -or
            [int]$row.applied_sesno -ne [int]$row.file_latest_sesno -or
            [int]$row.model_sesno -ne [int]$row.applied_sesno -or
            [int]$row.room_sesno -ne [int]$row.applied_sesno -or
            $row.model_verdict -ne 'in_sync') {
            $rowsDone = $false
        }
    }

    $done = $dataTasks.Count -gt 0 -and $terminal.Count -eq $dataTasks.Count -and $rowsDone `
        -and (Field $h 'initialization.data_ready') -and (Field $h 'initialization.model_ready') `
        -and $roomStatus -eq 'ready' -and -not $rebuildRequired
    if ($done) {
        Log "DONE after $([int]((Get-Date) - $started).TotalMinutes) min"
        $h | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $RunDir 'final-health.json') -Encoding UTF8
        $d | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $RunDir 'final-dbnums.json') -Encoding UTF8
        $t | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $RunDir 'final-tasks.json') -Encoding UTF8
        exit 0
    }

    $failed = @($dataTasks | Where-Object { $_.state -eq 'failed' -and [int]$_.dbnum -in $Dbnums })
    if ($failed.Count -gt 0 -and $terminal.Count -eq $dataTasks.Count) {
        Log "FAILED: $(($failed | ForEach-Object { "$($_.dbnum):$($_.task_id)" }) -join ' ')"
        $t | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $RunDir 'final-tasks.json') -Encoding UTF8
        exit 2
    }
    if ($Once) { Log "not done yet (single-shot)"; exit 1 }
    Start-Sleep -Seconds $PollSecs
}
Log "TIMEOUT after $TimeoutMins min"
exit 3
