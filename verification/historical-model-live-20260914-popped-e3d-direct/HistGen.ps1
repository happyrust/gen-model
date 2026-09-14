$script:ev = 'D:\work\plant-code\old\gen-model\verification\historical-model-live-20260914-popped-e3d-direct'
$script:base = 'http://127.0.0.1:8047'
New-Item -ItemType Directory -Force $script:ev | Out-Null

function Read-ErrorBody($exception) {
    $r = $exception.Response
    if (-not $r) { return $null }
    try {
        $sr = New-Object IO.StreamReader($r.GetResponseStream())
        return @{ status = [int]$r.StatusCode; body = $sr.ReadToEnd() }
    } catch { return @{ status = [int]$r.StatusCode; body = $exception.Message } }
}

function Invoke-HistGen($name, $body) {
    $json = $body | ConvertTo-Json -Compress
    $json | Set-Content "$script:ev\$name.request.json" -Encoding UTF8
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try {
        $resp = Invoke-WebRequest -Uri "$script:base/api/v1/model/history/generate" -Method Post -ContentType 'application/json' -Body $json -TimeoutSec 30
    } catch {
        $e = Read-ErrorBody $_.Exception
        if ($e) {
            "$name -> HTTP $($e.status): $($e.body)"
            $e.body | Set-Content "$script:ev\$name.error.json" -Encoding UTF8
        } else { "$name -> $($_.Exception.Message)" }
        return
    }
    $accepted = $resp.Content | ConvertFrom-Json
    "$name -> HTTP $($resp.StatusCode) task_id=$($accepted.task_id)"
    $task = $null
    foreach ($i in 1..600) {
        Start-Sleep -Milliseconds 500
        $task = Invoke-RestMethod "$script:base/api/v1/tasks/$($accepted.task_id)" -TimeoutSec 10
        if ($task.state -ne 'running' -and $task.state -ne 'queued') { break }
    }
    $sw.Stop()
    $task | ConvertTo-Json -Depth 8 | Set-Content "$script:ev\$name.task.json" -Encoding UTF8
    "$name -> state=$($task.state) kind=$($task.kind) wall=$([int]$sw.Elapsed.TotalMilliseconds)ms"
    if ($task.state -eq 'succeeded') {
        $r = $task.result
        "  snapshot_key=$($r.snapshot_key) resolved=$($r.resolved_session.requested) sesno=$($r.resolved_session.sesno) session_time=$($r.resolved_session.session_time)"
        "  visited=$($r.visited) generated=$($r.generated) failed=$($r.failed) skipped=$($r.skipped) upserted=$($r.upserted) shared=$($r.shared_instances) baked=$($r.baked_instances) mesh_written=$($r.mesh_written) mesh_reused=$($r.mesh_reused) unique_meshes=$($r.unique_meshes) elapsed_ms=$($r.elapsed_ms)"
    } else {
        "  result=$($task.result | ConvertTo-Json -Compress)"
    }
}

function Invoke-HistQuery($name, $key, $tool) {
    $json = @{ snapshot_key = $key; tool = $tool } | ConvertTo-Json -Compress
    try {
        $resp = Invoke-WebRequest -Uri "$script:base/api/v1/model/history/query" -Method Post -ContentType 'application/json' -Body $json -TimeoutSec 60
    } catch {
        $e = Read-ErrorBody $_.Exception
        "$name/$tool -> HTTP $($e.status): $($e.body)"
        return $null
    }
    $resp.Content | Set-Content "$script:ev\$name.query-$tool.json" -Encoding UTF8
    $rows = $resp.Content | ConvertFrom-Json
    "$name/$tool -> HTTP $($resp.StatusCode) rows=$(@($rows).Count) bytes=$($resp.Content.Length)"
    return $rows
}
