# 网页分块上传（/v1/services/upload）：两块收下、暂存看得到、单块超上限回 413、清掉暂存。
# 要求 mock 根已生成升级密钥（deploy-dash-agent.exe upgrade-key --root <Root>），否则回 403 upgrade_disabled。
param([int]$Port = 18691, [string]$Root = (Join-Path $PSScriptRoot '..\target\test-run\mock'), [string]$Service = 'rs_server')
$ErrorActionPreference = 'Stop'
$base = "http://127.0.0.1:$Port"
$tok = (Get-Content -Raw (Join-Path $Root 'run\gui\agent.token')).Trim()
$h = @{ Authorization = "Bearer $tok"; 'X-Client' = 'deploy-dash-tests\upload-check' }
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
function Bytes([int]$n) { $b = [byte[]]::new($n); $rng.GetBytes($b); , $b }
function Put([long]$off, [long]$total, [int]$reset, [int]$fin, [byte[]]$body) {
    $qs = "service=$Service&rel_path=rs-server.exe&offset=$off&total=$total&reset=$reset&finalize=$fin"
    $r = Invoke-WebRequest -Method PUT -Uri "$base/v1/services/upload?$qs" -Headers $h -Body $body -ContentType 'application/octet-stream' -UseBasicParsing -SkipHttpErrorCheck
    $j = try { $r.Content | ConvertFrom-Json } catch { $null }
    [pscustomobject]@{ s = $r.StatusCode; err = $j.error; msg = $j.message }
}
$ok = $true
function Check([string]$name, [bool]$cond, [string]$detail) {
    if (-not $cond) { $script:ok = $false }
    Write-Output ("{0} {1}  ({2})" -f $(if ($cond) { 'OK  ' } else { 'FAIL' }), $name, $detail)
}

$MiB = 1024 * 1024
$total = 3 * $MiB
$r1 = Put 0 $total 1 0 (Bytes (2 * $MiB))
Check '第一块 2 MiB（网页的块大小）收下' ($r1.s -eq 200) "HTTP $($r1.s) $($r1.err) $($r1.msg)"
$r2 = Put (2 * $MiB) $total 0 1 (Bytes (1 * $MiB))
Check '第二块 1 MiB + 收尾 finalize' ($r2.s -eq 200) "HTTP $($r2.s) $($r2.err) $($r2.msg)"

$v = (Invoke-WebRequest -Uri "$base/v1/services/versions" -Headers $h -UseBasicParsing -SkipHttpErrorCheck)
$staged = $null
try { $vj = $v.Content | ConvertFrom-Json; $e = @($vj.services | Where-Object { $_.service -eq $Service -or $_.name -eq $Service })[0]; $staged = $e.staged } catch { }
Check '版本对比里看得到暂存的内容' ($v.StatusCode -eq 200 -and $null -ne $staged) ("HTTP {0}；staged={1}" -f $v.StatusCode, $(if ($staged) { ($staged | ConvertTo-Json -Compress -Depth 3).Substring(0, [Math]::Min(160, ($staged | ConvertTo-Json -Compress -Depth 3).Length)) } else { '无' }))

$r3 = Put 0 (5 * $MiB) 1 0 (Bytes (5 * $MiB))
Check '一块 5 MiB 超上限 413' ($r3.s -eq 413) "HTTP $($r3.s) $($r3.err)"

$c = Invoke-WebRequest -Method POST -Uri "$base/v1/services/stage-clear" -Headers $h -Body ('{"service":"' + $Service + '"}') -ContentType 'application/json' -UseBasicParsing -SkipHttpErrorCheck
Check '清掉暂存' ($c.StatusCode -eq 200) "HTTP $($c.StatusCode)"
Write-Output $(if ($ok) { '== 全部通过' } else { '== 有失败项' })
