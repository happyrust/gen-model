# 服务在跑时单独停、起一个服务，别的服务不受影响；最后全部停止。对着 mock 代理跑，同 webcheck.ps1。
param([int]$Port = 18691, [string]$Root = (Join-Path $PSScriptRoot '..\target\test-run\mock'), [string]$Service = 'rs_server')
$ErrorActionPreference = 'Stop'
$base = "http://127.0.0.1:$Port"
$tok = (Get-Content -Raw (Join-Path $Root 'run\gui\agent.token')).Trim()
$h = @{ Authorization = "Bearer $tok"; 'X-Client' = 'deploy-dash-tests\webcheck-running' }

function Post([string]$body) {
    $r = Invoke-WebRequest -Method POST -Uri "$base/v1/jobs" -Headers $h -Body ([Text.Encoding]::UTF8.GetBytes($body)) -ContentType 'application/json; charset=utf-8' -UseBasicParsing -SkipHttpErrorCheck
    if ($r.StatusCode -ne 202) { throw "任务没收下：HTTP $($r.StatusCode) $($r.Content)" }
    ($r.Content | ConvertFrom-Json).job_id
}
function WaitJob([int]$id, [int]$secs = 30) {
    $deadline = (Get-Date).AddSeconds($secs)
    do {
        $j = (Invoke-WebRequest -Uri "$base/v1/jobs/$id" -Headers $h -UseBasicParsing).Content | ConvertFrom-Json
        if ($j.state -notin @('queued', 'running')) { return $j }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $deadline)
    $j
}
function States {
    $s = (Invoke-WebRequest -Uri "$base/v1/status?summary=1" -Headers $h -UseBasicParsing).Content | ConvertFrom-Json
    $m = [ordered]@{}
    foreach ($x in $s.services) { $m[[string]$x.name] = [string]$x.state }
    $m
}
function WaitState([string]$svc, [string]$want, [int]$secs = 10) {
    $deadline = (Get-Date).AddSeconds($secs)
    do { $st = States; if ($st[$svc] -eq $want) { return $st }; Start-Sleep -Milliseconds 300 } while ((Get-Date) -lt $deadline)
    $st
}
function Show($st) { ($st.GetEnumerator() | ForEach-Object { '{0}={1}' -f $_.Key, $_.Value }) -join ' ' }

$ok = $true
$r = Invoke-WebRequest -Method POST -Uri "$base/v1/debug/fixture" -Headers $h -Body '{"fixture":"running_healthy"}' -ContentType 'application/json' -UseBasicParsing -SkipHttpErrorCheck
$st = WaitState $Service 'ok'
$allUp = @($st.Values | Where-Object { $_ -ne 'ok' }).Count -eq 0
Write-Output ("{0} 切到「运行中」场景（HTTP {1}）：{2}" -f $(if ($r.StatusCode -eq 200 -and $allUp) { 'OK  ' } else { $ok = $false; 'FAIL' }), $r.StatusCode, (Show $st))

$id = Post ('{"kind":"stop","args":{"service":"' + $Service + '"},"confirm":true,"note":"测试"}')
$j = WaitJob $id
$st = WaitState $Service 'stopped'
$others = @($st.GetEnumerator() | Where-Object { $_.Key -ne $Service -and $_.Value -ne 'ok' }).Count -eq 0
Write-Output ("{0} 单独停 {1}（任务 {2}：{3} exit={4}）：{5}" -f $(if ($j.exit_code -eq 0 -and $st[$Service] -eq 'stopped' -and $others) { 'OK  ' } else { $ok = $false; 'FAIL' }), $Service, $id, $j.state, $j.exit_code, (Show $st))

$id = Post ('{"kind":"start","args":{"service":"' + $Service + '"},"note":"测试"}')
$j = WaitJob $id
$st = WaitState $Service 'ok' 25
Write-Output ("{0} 单独起 {1}（任务 {2}：{3} exit={4}）：{5}" -f $(if ($j.exit_code -eq 0 -and $st[$Service] -eq 'ok') { 'OK  ' } else { $ok = $false; 'FAIL' }), $Service, $id, $j.state, $j.exit_code, (Show $st))

$id = Post '{"kind":"down","args":{"force":false},"confirm":true,"note":"测试收尾"}'
$j = WaitJob $id
Write-Output ("收尾全部停止（任务 {0}：{1} exit={2}）：{3}" -f $id, $j.state, $j.exit_code, (Show (States)))
Write-Output $(if ($ok) { '== 全部通过' } else { '== 有失败项' })
