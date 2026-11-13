# 网页面板接口检查（14 项）：页面、令牌、单服务启停、CORS、老推程序接口的 403。
# 对着一个 mock 代理跑：deploy-dash-agent.exe --root <Root> --listen 127.0.0.1:<Port> --mock
param([int]$Port = 18691, [string]$Root = (Join-Path $PSScriptRoot '..\target\test-run\mock'), [string]$Service = 'rs_server')
$ErrorActionPreference = 'Stop'
$base = "http://127.0.0.1:$Port"
$tok = (Get-Content -Raw (Join-Path $Root 'run\gui\agent.token')).Trim()

function Call([string]$m, [string]$p, [string]$b = '', [hashtable]$extra = @{}, [switch]$NoToken) {
    $h = @{ 'X-Client' = 'deploy-dash-tests\webcheck' }
    if (-not $NoToken) { $h['Authorization'] = "Bearer $tok" }
    foreach ($k in $extra.Keys) { $h[$k] = $extra[$k] }
    $a = @{ Method = $m; Uri = "$base$p"; Headers = $h; UseBasicParsing = $true; TimeoutSec = 15; SkipHttpErrorCheck = $true }
    if ($b) { $a['Body'] = [Text.Encoding]::UTF8.GetBytes($b); $a['ContentType'] = 'application/json; charset=utf-8' }
    $r = Invoke-WebRequest @a
    $hdr = @{}
    foreach ($k in $r.Headers.Keys) { $hdr[$k] = ($r.Headers[$k] -join ', ') }
    [pscustomobject]@{ s = [int]$r.StatusCode; c = [string]$r.Content; h = $hdr }
}

function WaitJob([int]$id, [int]$secs = 25) {
    $deadline = (Get-Date).AddSeconds($secs)
    do {
        $j = (Call GET "/v1/jobs/$id").c | ConvertFrom-Json
        if ($j.state -notin @('queued', 'running')) { return $j }
        Start-Sleep -Milliseconds 300
    } while ((Get-Date) -lt $deadline)
    return $j
}

function SvcStates {
    $s = (Call GET '/v1/status?summary=1').c | ConvertFrom-Json
    ($s.services | ForEach-Object { '{0}={1}' -f $_.name, $_.state }) -join ' '
}

$ok = $true
function Check([string]$what, [bool]$cond, [string]$detail = '') {
    $mark = if ($cond) { 'OK  ' } else { $script:ok = $false; 'FAIL' }
    Write-Output ("{0} {1}{2}" -f $mark, $what, $(if ($detail) { "  ($detail)" } else { '' }))
}

$page = Call GET '/' -NoToken
$title = ([regex]'<title>([^<]*)</title>').Match($page.c).Groups[1].Value
Check '网页面板打开（GET / 不要令牌）' ($page.s -eq 200 -and $page.c.Length -gt 10000) "HTTP $($page.s), $($page.c.Length) 字符, title=$title"
$ping = Call GET '/v1/ping' -NoToken
Check 'ping 不要令牌' ($ping.s -eq 200) "HTTP $($ping.s)"
$info = Call GET '/v1/info'
$infoJ = $info.c | ConvertFrom-Json
Check '服务器信息（带令牌）' ($info.s -eq 200 -and $infoJ.mock) "HTTP $($info.s), mock=$($infoJ.mock), backend=$($infoJ.backend)"
$st = Call GET '/v1/status?summary=1'
Check '列状态（服务器栏用的 summary）' ($st.s -eq 200) "HTTP $($st.s): $(SvcStates)"

$r = Call POST '/v1/jobs' ('{"kind":"start","args":{"service":"' + $Service + '"},"note":"测试"}')
$id = if ($r.s -eq 202) { ($r.c | ConvertFrom-Json).job_id } else { 0 }
Check "单服务启动 $Service 收下" ($r.s -eq 202) "HTTP $($r.s) $(if ($r.s -ne 202) { $r.c })"
if ($id) { $j = WaitJob $id; Start-Sleep -Milliseconds 800; Check "单服务启动完成" ($j.state -eq 'done' -and $j.exit_code -eq 0) "任务 $id $($j.label)：$($j.state) exit=$($j.exit_code)；$(SvcStates)" }
$r = Call POST '/v1/jobs' ('{"kind":"stop","args":{"service":"' + $Service + '"},"confirm":true,"note":"测试"}')
$id = if ($r.s -eq 202) { ($r.c | ConvertFrom-Json).job_id } else { 0 }
Check "单服务停止 $Service 收下" ($r.s -eq 202) "HTTP $($r.s) $(if ($r.s -ne 202) { $r.c })"
if ($id) { $j = WaitJob $id; Start-Sleep -Milliseconds 800; Check "单服务停止完成" ($j.state -eq 'done' -and $j.exit_code -eq 0) "任务 $id $($j.label)：$($j.state) exit=$($j.exit_code)；$(SvcStates)" }

$r = Call POST '/v1/jobs' '{"kind":"doctor"}' -NoToken
Check '没带令牌的写操作 401' ($r.s -eq 401) "HTTP $($r.s)"
$origin = @{ Origin = 'http://127.0.0.1:18699' }
$r = Call GET '/v1/status?summary=1' '' $origin
Check '跨源带令牌：给 CORS 头' ($r.s -eq 200 -and $r.h['Access-Control-Allow-Origin'] -eq '*') "Allow-Origin=$($r.h['Access-Control-Allow-Origin'])"
$r = Call OPTIONS '/v1/jobs' '' (@{ Origin = 'http://127.0.0.1:18699'; 'Access-Control-Request-Method' = 'POST'; 'Access-Control-Request-Headers' = 'authorization,content-type' }) -NoToken
Check '预检 204 带放行头' ($r.s -eq 204 -and $r.h['Access-Control-Allow-Headers'] -match 'Authorization') "HTTP $($r.s), Allow-Headers=$($r.h['Access-Control-Allow-Headers'])"
$r = Call GET '/v1/status?summary=1'
Check '不带 Origin 不给 CORS 头' (-not $r.h.ContainsKey('Access-Control-Allow-Origin'))
$r = Call POST '/v1/services/upgrade' '{}'
$e = try { ($r.c | ConvertFrom-Json).error } catch { '' }
Check '没开升级时老的推程序接口 403' ($r.s -eq 403 -and $e -eq 'upgrade_disabled') "HTTP $($r.s) $e"
$r = Call GET '/v1/services/versions'
Check '版本对比（只读）照常' ($r.s -eq 200) "HTTP $($r.s)"
Write-Output $(if ($ok) { '== 全部通过' } else { '== 有失败项' })
