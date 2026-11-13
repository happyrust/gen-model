# 网页分块上传（/v1/services/upload）和应用：两块收下、暂存看得到、应用时真走「停 → 换 → 起」、单块超上限回 413、清掉暂存。
# mock 根先用 mock-root.ps1 -UpgradeKey 备好：没有升级密钥上传回 403 upgrade_disabled，没有假 stack.ps1 应用那几项不过。
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
$b1 = Bytes (2 * $MiB)
$b2 = Bytes (1 * $MiB)
$r1 = Put 0 $total 1 0 $b1
Check '第一块 2 MiB（网页的块大小）收下' ($r1.s -eq 200) "HTTP $($r1.s) $($r1.err) $($r1.msg)"
$r2 = Put (2 * $MiB) $total 0 1 $b2
Check '第二块 1 MiB + 收尾 finalize' ($r2.s -eq 200) "HTTP $($r2.s) $($r2.err) $($r2.msg)"

$v = (Invoke-WebRequest -Uri "$base/v1/services/versions" -Headers $h -UseBasicParsing -SkipHttpErrorCheck)
$staged = $null
try { $vj = $v.Content | ConvertFrom-Json; $e = @($vj.services | Where-Object { $_.service -eq $Service -or $_.name -eq $Service })[0]; $staged = $e.staged } catch { }
Check '版本对比里看得到暂存的内容' ($v.StatusCode -eq 200 -and $null -ne $staged) ("HTTP {0}；staged={1}" -f $v.StatusCode, $(if ($staged) { ($staged | ConvertTo-Json -Compress -Depth 3).Substring(0, [Math]::Min(160, ($staged | ConvertTo-Json -Compress -Depth 3).Length)) } else { '无' }))

# 应用：mock 根装了假 stack.ps1 时要依次走「停 → 换 → 起 → 就绪」，文件换成刚传的两块。
# 不是假脚本就不调应用接口：-Root、-Port 指错了也不会真升了别处的服务。
$stackPs1 = Join-Path $Root 'stack.ps1'
if ((Test-Path -LiteralPath $stackPs1) -and (Select-String -LiteralPath $stackPs1 -SimpleMatch 'deploy-dash-fake-stack' -Quiet)) {
    $a = Invoke-WebRequest -Method POST -Uri "$base/v1/services/upgrade" -Headers $h -Body ('{"service":"' + $Service + '","force":false,"auto_start":true}') -ContentType 'application/json' -UseBasicParsing -SkipHttpErrorCheck
    $aj = try { $a.Content | ConvertFrom-Json } catch { $null }
    $logs = @($aj.logs)
    function At([string]$pat) { for ($i = 0; $i -lt $logs.Count; $i++) { if ($logs[$i] -match $pat) { return $i } }; -1 }
    Check '应用升级' ($a.StatusCode -eq 200) "HTTP $($a.StatusCode) $($aj.error) $($aj.message)"
    $iStop = At "^\[fake-stack\] stop $Service$"; $iSwap = At '\] 3/4 '; $iStart = At "^\[fake-stack\] start $Service$"; $iReady = At "^\[fake-stack\] ready $Service$"
    Check '依次走了停 → 换 → 起 → 就绪' ($iStop -ge 0 -and $iStop -lt $iSwap -and $iSwap -lt $iStart -and $iStart -lt $iReady) "日志第 $iStop / $iSwap / $iStart / $iReady 行（共 $($logs.Count) 行，-1 是没找到）"
    $skipped = @($logs | Where-Object { $_ -match '跳过停服务|保持停止状态' })
    Check '没走「跳过停、起」那条路' ($skipped.Count -eq 0) $(if ($skipped.Count) { $skipped -join ' | ' } else { '无' })
    $targetRel = @{ database = 'bin\surreal.exe'; backend = 'bin\aios-database.exe'; rs_server = 'bin\rs-server.exe' }[$Service]
    if ($targetRel) {
        $sha = [Security.Cryptography.SHA256]::Create()
        [void]$sha.TransformBlock($b1, 0, $b1.Length, $null, 0)
        [void]$sha.TransformFinalBlock($b2, 0, $b2.Length)
        $want = ([BitConverter]::ToString($sha.Hash) -replace '-', '').ToLower()
        $target = Join-Path $Root $targetRel
        $got = if (Test-Path -LiteralPath $target) { (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLower() } else { '没有文件' }
        Check '目标文件换成了上传的内容' ($got -eq $want) "$targetRel sha256 $($got.Substring(0, [Math]::Min(12, $got.Length)))，应为 $($want.Substring(0, 12))"
    }
} else {
    Check '应用升级（mock 根要先跑 mock-root.ps1 装假 stack.ps1）' $false "$stackPs1 不是测试用的假脚本，没调应用接口"
}

$r3 = Put 0 (5 * $MiB) 1 0 (Bytes (5 * $MiB))
Check '一块 5 MiB 超上限 413' ($r3.s -eq 413) "HTTP $($r3.s) $($r3.err)"

$c = Invoke-WebRequest -Method POST -Uri "$base/v1/services/stage-clear" -Headers $h -Body ('{"service":"' + $Service + '"}') -ContentType 'application/json' -UseBasicParsing -SkipHttpErrorCheck
Check '清掉暂存' ($c.StatusCode -eq 200) "HTTP $($c.StatusCode)"
Write-Output $(if ($ok) { '== 全部通过' } else { '== 有失败项' })
