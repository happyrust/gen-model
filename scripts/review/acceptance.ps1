<#
.SYNOPSIS
    校审移植验收 runner：起真服务，把计划 §6 里不依赖浏览器的判据实跑一遍并留痕。

.DESCRIPTION
    依据 docs/plans/2026-09-12-review-backend-port-plan.md §6 与 docs/adr/ADR-072-review-data-persistence.md。

    这是一个**有尽头**的脚本：起 SurrealDB + gen-model → 等 health → 逐项探 → 停再起复读（A6）
    → 用内存介质再起一次看拒绝横幅（A9）→ 收掉全部自己起的子进程 → 打印汇总后退出。
    命令返回时不该再有本脚本起的进程活着；每一步都有超时兜底，任何一步卡住都不会把整跑拖死。

    端口不是契约：8022 / 8009 被占就自动换空闲端口，实际用的端口打印在汇总里、也写进 -JsonOut。
    **不会去 kill 任何不是本脚本起的进程**——这台机器上同时跑着别的项目。

.PARAMETER ApiPort
    gen-model 的 HTTP 端口首选值；被占则自动顺延。默认 8022。

.PARAMETER DbPort
    SurrealDB 的端口首选值；被占则自动顺延。默认 8009。

.PARAMETER LegacyBaseUrl
    旧端 plant-model-gen 的地址，用于 A2 / A5 / A10 两端对比。探不通就把那几条标 NOT RUN。

.EXAMPLE
    . .\scripts\review\env.ps1
    .\scripts\review\acceptance.ps1 -JsonOut .\.tmp-acceptance.json
#>
[CmdletBinding()]
param(
    [int]    $ApiPort = 8022,
    [int]    $DbPort = 8009,
    [string] $LegacyBaseUrl = 'http://127.0.0.1:3100',
    [string] $LegacySourceEvidence = '',
    [string] $Exe = 'D:\Rust\target-review-port\debug\aios-database.exe',
    [string] $SurrealExe = 'D:\work\plant-code\old\gen-model\bin\surreal.exe',
    [string] $BashExe = 'E:\Program Files\Git\bin\bash.exe',
    [string] $OptionTemplate = 'db_options\DbOption-e2e-8009.toml',
    [int]    $HealthTimeoutSec = 120,
    [int]    $StepTimeoutSec = 90,
    [string] $ProjectId = '2410',
    [string] $JsonOut,
    [switch] $KeepWorkDir
)

#requires -Version 7.0
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ── 全局状态 ──────────────────────────────────────────────────────────────
$script:Spawned = @()          # 只装本脚本起的进程；清理只碰这一份名单
$script:Results = [ordered]@{} # 判据 → @{ verdict; command; excerpt; note }
$script:A6TaskId = $null       # StrictMode 下先声明，失败早退时 finally 才读得到
$script:RunId = (Get-Date).ToString('yyyy-MM-ddTHH-mm-ss')
$script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$script:WorkDir = Join-Path $env:TEMP "review-acceptance-$script:RunId"

function Write-Step($text) { Write-Host "`n── $text ──" -ForegroundColor Cyan }
function Write-Info($text) { Write-Host "   $text" }
function Write-Ok($text) { Write-Host "   PASS  $text" -ForegroundColor Green }
function Write-Bad($text) { Write-Host "   FAIL  $text" -ForegroundColor Red }
function Write-Skip($text) { Write-Host "   NOT RUN  $text" -ForegroundColor Yellow }

function Set-Result {
    param([string]$Id, [string]$Verdict, [string]$Command = '', [string]$Excerpt = '', [string]$Note = '')
    $script:Results[$Id] = [ordered]@{
        verdict = $Verdict; command = $Command; excerpt = $Excerpt; note = $Note
    }
    switch ($Verdict) {
        'PASS' { Write-Ok "$Id  $Note" }
        'FAIL' { Write-Bad "$Id  $Note" }
        default { Write-Skip "$Id  $Note" }
    }
}

function Test-PortBusy([int]$Port) {
    $c = New-Object Net.Sockets.TcpClient
    try { $c.Connect('127.0.0.1', $Port); return $true } catch { return $false } finally { $c.Dispose() }
}

function Get-FreePort([int]$Preferred, [int]$Fallback) {
    if (-not (Test-PortBusy $Preferred)) { return $Preferred }
    Write-Info "端口 $Preferred 已被占用（不是本脚本起的，不动它），顺延找空闲口…"
    for ($p = $Fallback; $p -lt ($Fallback + 40); $p++) {
        if (-not (Test-PortBusy $p)) { return $p }
    }
    throw "从 $Fallback 起连续 40 个端口都被占，换 -ApiPort / -DbPort 指定"
}

function Start-Tracked {
    # 形参别叫 $Args：它是 PowerShell 的自动变量，函数里会被实参数组覆盖掉，
    # 结果 Start-Process 一个参数都收不到、子进程打印 usage 就退出（2026-09-13 踩过）。
    param([string]$FilePath, [string[]]$ArgumentList, [string]$WorkingDirectory, [string]$StdOut, [string]$StdErr, [hashtable]$Env)
    $previous = @{}
    if ($Env) {
        foreach ($k in $Env.Keys) {
            $previous[$k] = [Environment]::GetEnvironmentVariable($k)
            [Environment]::SetEnvironmentVariable($k, $Env[$k])
        }
    }
    try {
        $sp = @{
            FilePath               = $FilePath
            WorkingDirectory       = $WorkingDirectory
            RedirectStandardOutput = $StdOut
            RedirectStandardError  = $StdErr
            PassThru               = $true
            WindowStyle            = 'Hidden'
        }
        # Start-Process 的 -ArgumentList 不收空数组，无参进程要整个省掉这一格。
        if ($ArgumentList -and $ArgumentList.Count -gt 0) { $sp['ArgumentList'] = $ArgumentList }
        $p = Start-Process @sp
    } finally {
        foreach ($k in $previous.Keys) { [Environment]::SetEnvironmentVariable($k, $previous[$k]) }
    }
    $script:Spawned += $p
    return $p
}

function Stop-Tracked([System.Diagnostics.Process]$Proc) {
    if (-not $Proc) { return }
    if ($Proc.HasExited) { return }
    # /t 连子进程一起收：cargo / npm 那类包装进程 SIGTERM 只杀得掉外层。
    & taskkill.exe /pid $Proc.Id /t /f 2>&1 | Out-Null
    $null = $Proc.WaitForExit(10000)
}

function Stop-AllTracked {
    foreach ($p in $script:Spawned) { try { Stop-Tracked $p } catch { } }
    $script:Spawned = @()
}

function Wait-Http {
    param([string]$Url, [int]$TimeoutSec, [int]$ExpectStatus = 200)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri $Url -TimeoutSec 5 -ErrorAction Stop
            if ($r.StatusCode -eq $ExpectStatus) { return $r }
        } catch {
            $status = $null
            try { $status = [int]$_.Exception.Response.StatusCode } catch { }
            if ($status -eq $ExpectStatus) { return $_.Exception.Response }
        }
        Start-Sleep -Milliseconds 500
    }
    return $null
}

function Invoke-Api {
    param([string]$Url, [string]$Method = 'GET', $Body, [string]$Token, [int]$TimeoutSec = 20)
    $headers = @{}
    if ($Token) { $headers['Authorization'] = "Bearer $Token" }
    $params = @{ Uri = $Url; Method = $Method; Headers = $headers; TimeoutSec = $TimeoutSec; ErrorAction = 'Stop' }
    if ($null -ne $Body) {
        $params['Body'] = ($Body | ConvertTo-Json -Depth 10 -Compress)
        $params['ContentType'] = 'application/json; charset=utf-8'
    }
    try {
        $r = Invoke-WebRequest @params
        return [pscustomobject]@{ Status = [int]$r.StatusCode; Body = $r.Content }
    } catch {
        # PowerShell 7 抛的是 HttpResponseException：响应体在 $_.ErrorDetails.Message，
        # 不在 Response.GetResponseStream()（那是 Windows PowerShell 5.1 的形制）。
        # 读错了这一格，4xx/5xx 的 JSON 正文就全成了异常文案，A3 的二次判定会把
        # handler 级 404 误判成「路由缺失」（2026-09-13 踩过）。
        $status = 0
        $body = ''
        try { $status = [int]$_.Exception.Response.StatusCode } catch { }
        if ($_.ErrorDetails -and $_.ErrorDetails.Message) { $body = $_.ErrorDetails.Message }
        if (-not $body) {
            try {
                $stream = $_.Exception.Response.GetResponseStream()
                $body = (New-Object IO.StreamReader($stream)).ReadToEnd()
            } catch { }
        }
        if (-not $body) { $body = $_.Exception.Message }
        return [pscustomobject]@{ Status = $status; Body = $body }
    }
}

function Head([string]$Text, [int]$Lines = 12) {
    if (-not $Text) { return '' }
    ($Text -split "`r?`n" | Select-Object -First $Lines) -join "`n"
}

# ── 主流程 ────────────────────────────────────────────────────────────────
$started = Get-Date
$legacyUp = $false
$apiBase = $null
$head = '<unknown>'
$dirty = @()

try {
    Push-Location $script:RepoRoot

    Write-Step "准备（run $script:RunId）"
    foreach ($pair in @(@{n = 'gen-model 二进制'; p = $Exe }, @{n = 'surreal'; p = $SurrealExe }, @{n = 'Git Bash'; p = $BashExe })) {
        if (-not (Test-Path $pair.p)) {
            throw "$($pair.n) 不在：$($pair.p)。gen-model 的 debug 二进制先跑：. .\scripts\review\env.ps1; cargo build --features http_api --bin aios-database"
        }
    }
    $head = (& git rev-parse --short HEAD).Trim()
    $dirty = @(& git status --porcelain)
    Write-Info "HEAD $head；工作树未提交 $($dirty.Count) 个文件"
    if ($dirty.Count -gt 0) { $dirty | ForEach-Object { Write-Info "  $_" } }

    $ApiPort = Get-FreePort $ApiPort 8122
    $DbPort = Get-FreePort $DbPort 8109
    $apiBase = "http://127.0.0.1:$ApiPort"
    Write-Info "gen-model → $apiBase ；SurrealDB → 127.0.0.1:$DbPort"

    $dataDir = Join-Path $script:WorkDir 'surreal-data'
    $logDir = Join-Path $script:WorkDir 'logs'
    New-Item -ItemType Directory -Force -Path $dataDir, $logDir | Out-Null
    Write-Info "工作目录 $script:WorkDir（数据与日志都在这里，不入库）"

    # DbOption.toml：从夹具复制到工作目录再改端口。夹具本身只读。
    $optionText = Get-Content (Join-Path $script:RepoRoot $OptionTemplate) -Raw
    $optionText = $optionText -replace '(?m)^v_port\s*=.*$', "v_port = $DbPort"
    $optionText = $optionText -replace '(?m)^http_api_addr\s*=.*$', "http_api_addr = `"127.0.0.1:$ApiPort`""
    $optionPath = Join-Path $script:WorkDir 'DbOption.toml'
    Set-Content -Path $optionPath -Value $optionText -Encoding UTF8
    Write-Info "DbOption.toml 已按实际端口改写（源夹具 $OptionTemplate 未动）"

    # 启动序列按**工作目录**读 resource/surreal 与 rs_surreal（lib.rs:937、embedded_surql.rs 头注释）。
    # 工作目录是临时目录，得把这两份资源带过来；缺了 resource/surreal 时连接阶段直接
    # 「Failed to define common functions: 系统找不到指定的路径 (os error 3)」（2026-09-13 踩过）。
    foreach ($res in @('resource', 'rs_surreal')) {
        $src = Join-Path $script:RepoRoot $res
        if (Test-Path $src) { Copy-Item -Recurse -Force $src (Join-Path $script:WorkDir $res) }
    }
    Write-Info 'resource/ 与 rs_surreal/ 已复制进工作目录（仓库侧只读）'

    # ── 起 SurrealDB ──────────────────────────────────────────────────────
    Write-Step '起 SurrealDB'
    $surreal = Start-Tracked -FilePath $SurrealExe `
        -ArgumentList @('start', '--user', 'root', '--pass', 'root', '--bind', "127.0.0.1:$DbPort", "rocksdb:$dataDir") `
        -WorkingDirectory $script:WorkDir `
        -StdOut (Join-Path $logDir 'surreal.out.log') -StdErr (Join-Path $logDir 'surreal.err.log')
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not (Test-PortBusy $DbPort)) { Start-Sleep -Milliseconds 300 }
    if (-not (Test-PortBusy $DbPort)) { throw "SurrealDB 60s 内没听上 $DbPort；看 $logDir\surreal.err.log" }
    Write-Info "SurrealDB pid $($surreal.Id) 就绪"

    # ── 起 gen-model ──────────────────────────────────────────────────────
    Write-Step '起 gen-model（durable 介质）'
    $api = Start-Tracked -FilePath $Exe -ArgumentList @() -WorkingDirectory $script:WorkDir `
        -StdOut (Join-Path $logDir 'api.out.log') -StdErr (Join-Path $logDir 'api.err.log')
    $healthRes = Wait-Http -Url "$apiBase/api/review/health" -TimeoutSec $HealthTimeoutSec
    if (-not $healthRes) {
        $tail = (Get-Content (Join-Path $logDir 'api.err.log') -Tail 30 -ErrorAction SilentlyContinue) -join "`n"
        throw "gen-model ${HealthTimeoutSec}s 内没把 /api/review/health 拉到 200。日志尾：`n$tail"
    }
    Write-Info "gen-model pid $($api.Id) 就绪"

    # ── 取 Bearer（A3 的二次判定与后面几条都要）────────────────────────────
    $tokenRes = Invoke-Api -Url "$apiBase/api/auth/token" -Method POST -Body @{
        project_id = 'acceptance'; user_id = 'SJ'; user_name = '设计'; role = 'sj'
    }
    $token = $null
    if ($tokenRes.Status -eq 200) { $token = ($tokenRes.Body | ConvertFrom-Json).data.token }
    Write-Info ($token ? 'Bearer 已取到' : "取 Bearer 失败（$($tokenRes.Status)）：带鉴权的判据会照实记")

    # ── A2 health ─────────────────────────────────────────────────────────
    Write-Step 'A2 /api/review/health 键集'
    $healthBody = $healthRes.Content
    $healthKeys = ($healthBody | ConvertFrom-Json).PSObject.Properties.Name | Sort-Object
    $legacyProbe = Invoke-Api -Url "$LegacyBaseUrl/api/review/health" -TimeoutSec 5
    # 仅端口可达不能证明运行的是批准的旧端；须显式提供启动/版本留痕路径。
    $legacyUp = ($legacyProbe.Status -eq 200 -and $LegacySourceEvidence -and (Test-Path -LiteralPath $LegacySourceEvidence))
    if ($legacyUp) {
        $legacyKeys = ($legacyProbe.Body | ConvertFrom-Json).PSObject.Properties.Name | Sort-Object
        $same = -not (Compare-Object $healthKeys $legacyKeys)
        Set-Result 'A2' ($same ? 'PASS' : 'FAIL') "curl $apiBase/api/review/health ；curl $LegacyBaseUrl/api/review/health" `
            "gen-model: $($healthKeys -join ',')`nlegacy  : $($legacyKeys -join ',')" `
            ($same ? '两端键集相同' : '两端键集不同')
    } else {
        Set-Result 'A2' 'PARTIAL' "curl $apiBase/api/review/health" (Head $healthBody 6) `
            "gen-model 侧 200、键集 [$($healthKeys -join ',')]；旧端 $LegacyBaseUrl 不可达（$($legacyProbe.Status)），两端对比 NOT RUN"
    }

    # ── A3 路由探针 ───────────────────────────────────────────────────────
    Write-Step 'A3 契约路由非 404（test_review_api.sh routes）'
    $a3Log = Join-Path $logDir 'a3-routes.log'
    & $BashExe './scripts/review/test_review_api.sh' $apiBase 'routes' *>&1 | Tee-Object -FilePath $a3Log | Out-Null
    $a3Text = (Get-Content $a3Log -Raw -ErrorAction SilentlyContinue)
    $a3Tail = ($a3Text -split "`r?`n" | Select-Object -Last 8) -join "`n"
    $a3Verdict = if ($LASTEXITCODE -eq 0) { 'PASS' } else { 'FAIL' }
    Set-Result 'A3' $a3Verdict "bash ./scripts/review/test_review_api.sh $apiBase routes" $a3Tail '路由判据由脚本统一实现；非零退出码不再由 runner 覆盖。'

    # ── A4 下载半边 ───────────────────────────────────────────────────────
    Write-Step 'A4 附件 multipart 上传 → 静态地址回读'
    $png = Join-Path $script:WorkDir 'a4.png'
    [IO.File]::WriteAllBytes($png, [Convert]::FromBase64String(
            'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+iS9cAAAAASUVORK5CYII='))
    $taskRes = Invoke-Api -Url "$apiBase/api/review/tasks" -Method POST -Token $token -Body @{
        title = "A4 验收任务 $script:RunId"; description = 'acceptance runner'; modelName = 'acceptance'; priority = 'medium'
    }
    $taskId = $null
    if ($taskRes.Status -lt 400) { $taskId = ($taskRes.Body | ConvertFrom-Json).task.id }
    if (-not $taskId) {
        Set-Result 'A4' 'FAIL' "POST $apiBase/api/review/tasks" (Head $taskRes.Body 6) "建任务失败（$($taskRes.Status)），附件没法挂"
    } else {
        $curlArgs = @('-s', '-o', (Join-Path $logDir 'a4-upload.json'), '-w', '%{http_code}',
            '-X', 'POST', "$apiBase/api/review/attachments",
            '-F', "taskId=$taskId", '-F', 'fileType=markup', '-F', 'description=A4 验收附件',
            '-F', "file=@$png;type=image/png")
        if ($token) { $curlArgs = @('-H', "Authorization: Bearer $token") + $curlArgs }
        $upCode = (& curl.exe @curlArgs)
        $upBody = Get-Content (Join-Path $logDir 'a4-upload.json') -Raw -ErrorAction SilentlyContinue
        $attUrl = $null
        try { $attUrl = ($upBody | ConvertFrom-Json).attachment.url } catch { }
        if (-not $attUrl) {
            Set-Result 'A4' 'FAIL' "curl -F file=@a4.png $apiBase/api/review/attachments" (Head $upBody 6) "上传回 $upCode，没拿到 attachment.url"
        } else {
            $dlHeaders = Join-Path $logDir 'a4-download.headers'
            $dlBody = Join-Path $logDir 'a4-download.bin'
            # 参数必须是数组再 splat：写成 `& curl.exe '-s', '-D', $x` 会被当成一个数组实参，
            # curl 收到的是拼起来的一坨，回 `URL rejected: Bad hostname`（2026-09-13 踩过）。
            $dlArgs = @('-s', '-D', $dlHeaders, '-o', $dlBody, '-w', '%{http_code}', "$apiBase$attUrl")
            $dlCode = (& curl.exe @dlArgs)
            $dlItem = Get-Item $dlBody -ErrorAction SilentlyContinue
            $bytes = if ($dlItem) { $dlItem.Length } else { 0 }
            $headersText = Get-Content $dlHeaders -Raw -ErrorAction SilentlyContinue
            $disp = ($headersText -split "`r?`n" | Where-Object { $_ -match '(?i)^content-disposition:' }) -join ''
            $ok = ($dlCode -eq '200' -and $bytes -gt 0)
            Set-Result 'A4' ($ok ? 'PARTIAL' : 'FAIL') "curl -F file=@a4.png .../api/review/attachments ；curl $apiBase$attUrl" `
                "upload HTTP $upCode → $attUrl`ndownload HTTP $dlCode，$bytes 字节`n$($disp ? $disp : 'Content-Disposition: <未返回>')" `
                ($ok ? "上传成功且静态地址回读 $bytes 字节；浏览器走查须另验，A4 尚未完整通过" : "静态回读失败（HTTP $dlCode）")
        }
        $script:A6TaskId = $taskId
    }

    # ── A5 401/403 文本 ───────────────────────────────────────────────────
    Write-Step 'A5 401/403 文本（test_review_api.sh auth）'
    $a5Log = Join-Path $logDir 'a5-auth.log'
    & $BashExe './scripts/review/test_review_api.sh' $apiBase 'auth' *>&1 | Tee-Object -FilePath $a5Log | Out-Null
    $a5Exit = $LASTEXITCODE
    $a5Text = Get-Content $a5Log -Raw -ErrorAction SilentlyContinue
    $a5Tail = ($a5Text -split "`r?`n" | Select-Object -Last 10) -join "`n"
    if ($legacyUp) {
        $a5LegacyLog = Join-Path $logDir 'a5-auth-legacy.log'
        & $BashExe './scripts/review/test_review_api.sh' $LegacyBaseUrl 'auth' *>&1 | Tee-Object -FilePath $a5LegacyLog | Out-Null
        $a5LegacyExit = $LASTEXITCODE
        $diff = Compare-Object @(Get-Content $a5Log | Where-Object { $_ -match 'HTTP|error_message|401|403' }) @(Get-Content $a5LegacyLog | Where-Object { $_ -match 'HTTP|error_message|401|403' })
        if ($a5Exit -ne 0 -or $a5LegacyExit -ne 0) { $diff = @('auth script failed') }
        Set-Result 'A5' ($diff ? 'FAIL' : 'PASS') "bash ./scripts/review/test_review_api.sh <base> auth（两端）" $a5Tail `
            ($diff ? "两端 auth 输出有 $($diff.Count) 处差异，见 a5-auth*.log" : '两端 auth 输出逐行相同')
    } else {
        Set-Result 'A5' ($a5Exit -eq 0 ? 'PARTIAL' : 'FAIL') "bash ./scripts/review/test_review_api.sh $apiBase auth" $a5Tail `
            "gen-model 单边已跑；旧端不可达，两端 diff NOT RUN（文本逐字断言由 auth.rs 的护栏测试兜着）"
    }

    # ── A10 模型侧集成 ────────────────────────────────────────────────────
    Write-Step 'A10 aux-data / collision-data'
    # aux-data 走 UCode / UKey 口令面（integration.rs:230-245），不是 Bearer；
    # UKey 缺省是**空串**，Invoke-WebRequest 会把空值头丢掉，所以这一条用 curl 的 `-H "UKey;"`。
    # 请求体形状取自该文件的 `tests::sample_request_body`（旧端原样）。
    $auxBodyFile = Join-Path $logDir 'a10-aux-request.json'
    @{
        project_id = $ProjectId; model_refnos = @('24381_1001'); major = 'pipe'
        requester_id = 'user-001'; page = 1; page_size = 20; form_id = "A10-$script:RunId"
    } | ConvertTo-Json -Compress | Set-Content -Path $auxBodyFile -Encoding UTF8
    $ucode = if ($env:AUX_DATA_UCODE) { $env:AUX_DATA_UCODE } else { 'ZY' }
    $ukeyHeader = if ($env:AUX_DATA_UKEY) { "UKey: $($env:AUX_DATA_UKEY)" } else { 'UKey;' }
    $auxOut = Join-Path $logDir 'a10-aux.json'
    $auxArgs = @('-s', '-o', $auxOut, '-w', '%{http_code}', '-X', 'POST',
        '-H', 'Content-Type: application/json', '-H', "UCode: $ucode", '-H', $ukeyHeader,
        '--data-binary', "@$auxBodyFile", "$apiBase/api/review/aux-data")
    $auxCode = (& curl.exe @auxArgs)
    $auxBody = Get-Content $auxOut -Raw -ErrorAction SilentlyContinue
    $coll = Invoke-Api -Url "$apiBase/api/review/collision-data?project_id=$ProjectId&limit=10" -Method GET
    # A10 的判据是「形状与旧端一致；无数据源时显式空集 + reason」。gen-model 半边能自证的是：
    # 两端点在册、口令面放行后 200、collision-data 带 reason 而不是静默空集。
    $auxOkShape = ($auxCode -eq '200' -and $auxBody -match '"code"\s*:\s*200' -and $auxBody -match '"data"')
    $collOkShape = ($coll.Status -eq 200 -and $coll.Body -match '"success"\s*:\s*true' -and $coll.Body -match '"reason"')
    $a10Ok = $auxOkShape -and $collOkShape
    Set-Result 'A10' ($a10Ok ? 'PARTIAL' : 'FAIL') `
        "curl -H 'UCode: $ucode' -H '$ukeyHeader' --data-binary @a10-aux-request.json $apiBase/api/review/aux-data ；curl '$apiBase/api/review/collision-data?project_id=$ProjectId&limit=10'" `
        "aux-data HTTP $($auxCode)：$(Head $auxBody 3)`ncollision-data HTTP $($coll.Status)：$(Head $coll.Body 3)" `
        ($a10Ok ? 'gen-model 半边形状对；尚未执行旧端逐字段对拍，不能由旧端 health 可达推断 PASS' : "形状不符：aux-data $auxCode / collision-data $($coll.Status)")

    # ── A6 重启后复读 ─────────────────────────────────────────────────────
    Write-Step 'A6 重启 gen-model 与 SurrealDB 后复读'
    if (-not $script:A6TaskId) {
        Set-Result 'A6' 'NOT RUN' '' '' '前面没建成任务，没有可复读的行'
    } else {
        $persistenceState = Join-Path $script:WorkDir 'persistence.json'
        $seedLog = @(& node "$PSScriptRoot/persistence.mjs" seed $apiBase $persistenceState 2>&1)
        if ($LASTEXITCODE -ne 0) { throw "A6 seed failed: $($seedLog -join [Environment]::NewLine)" }
        Stop-Tracked $api
        Stop-Tracked $surreal
        $script:Spawned = @()
        Start-Sleep -Seconds 2

        $surreal = Start-Tracked -FilePath $SurrealExe `
            -ArgumentList @('start', '--user', 'root', '--pass', 'root', '--bind', "127.0.0.1:$DbPort", "rocksdb:$dataDir") `
            -WorkingDirectory $script:WorkDir `
            -StdOut (Join-Path $logDir 'surreal.restart.out.log') -StdErr (Join-Path $logDir 'surreal.restart.err.log')
        $deadline = (Get-Date).AddSeconds(60)
        while ((Get-Date) -lt $deadline -and -not (Test-PortBusy $DbPort)) { Start-Sleep -Milliseconds 300 }

        $api = Start-Tracked -FilePath $Exe -ArgumentList @() -WorkingDirectory $script:WorkDir `
            -StdOut (Join-Path $logDir 'api.restart.out.log') -StdErr (Join-Path $logDir 'api.restart.err.log')
        $back = Wait-Http -Url "$apiBase/api/review/health" -TimeoutSec $HealthTimeoutSec
        if (-not $back) {
            Set-Result 'A6' 'FAIL' '' '' "重启后 ${HealthTimeoutSec}s 内没恢复 health"
        } else {
            $verifyLog = @(& node "$PSScriptRoot/persistence.mjs" verify $apiBase $persistenceState 2>&1)
            $ok = $LASTEXITCODE -eq 0
            Set-Result 'A6' ($ok ? 'PASS' : 'FAIL') "node scripts/review/persistence.mjs seed|verify <base> <state>（双重启前后）" `
                (($seedLog + $verifyLog) -join [Environment]::NewLine) '任务、记录、评论、批注状态、历史、工作流、附件字节哈希均非空且完全一致'

        }
    }

    # ── A9 两种内存介质：拒绝校审，模型健康与树接口仍可服务 ──
    Stop-AllTracked
    $a9Results = @()
    foreach ($mode in @('embedded-mem', 'spawned-mem')) {
        Write-Step "A9 $mode"
        $a9Port = Get-FreePort ($ApiPort + 1) 8222
        $a9DbPort = Get-FreePort ($DbPort + 1) 8223
        $a9Dir = Join-Path $script:WorkDir "a9-$mode"
        New-Item -ItemType Directory -Force -Path $a9Dir | Out-Null
        foreach ($res in @('resource', 'rs_surreal')) {
            $src = Join-Path $script:RepoRoot $res
            if (Test-Path $src) { Copy-Item -Recurse -Force $src (Join-Path $a9Dir $res) }
        }
        $a9Option = $optionText -replace '(?m)^http_api_addr\s*=.*$', "http_api_addr = `"127.0.0.1:$a9Port`""
        $a9Option = $a9Option -replace '(?m)^v_port\s*=.*$', "v_port = $a9DbPort"
        Set-Content (Join-Path $a9Dir 'DbOption.toml') $a9Option -Encoding UTF8
        $a9Out = Join-Path $logDir "a9-$mode.out.log"
        $a9Err = Join-Path $logDir "a9-$mode.err.log"
        $proc = Start-Tracked -FilePath $Exe -WorkingDirectory $a9Dir -StdOut $a9Out -StdErr $a9Err `
            -Env @{ AIOS_STORE_MODE = $mode; AIOS_SURREAL_EXE = $SurrealExe; PLANT_REVIEW_ALLOW_EPHEMERAL_STORE = '' }
        $base = "http://127.0.0.1:$a9Port"
        $null = Wait-Http "$base/api/review/health" $StepTimeoutSec 503
        $checks = @()
        foreach ($path in @('/api/review/health', '/api/users', '/api/auth/token', '/files/review_attachments/probe.png')) {
            $r = Invoke-Api "$base$path"
            $checks += ($r.Status -eq 503 -and $r.Body -match 'review.persistence.refused')
        }
        $modelOk = $false
        $modelDeadline = (Get-Date).AddSeconds($StepTimeoutSec)
        do {
            $model = Invoke-Api "$base/api/v1/health"
            if ($model.Status -eq 200) {
                $m = $model.Body | ConvertFrom-Json
                $modelOk = $m.status -eq 'ok' -and $m.project -and $m.mdb
            }
            if (!$modelOk) { Start-Sleep -Seconds 1 }
        } while (!$modelOk -and (Get-Date) -lt $modelDeadline -and !$proc.HasExited)
        $log = (Get-Content $a9Out -Raw) + (Get-Content $a9Err -Raw)
        $banner = $log -match '拒绝启用'
        $ok = $banner -and $modelOk -and ($checks -notcontains $false)
        $item = [ordered]@{ mode = $mode; passed = $ok; refusedPrefixes = $checks; banner = $banner; modelHttp = $model.Status; modelHealthy = $modelOk; exited = $proc.HasExited }
        $a9Results += $item
        $model.Body | Set-Content (Join-Path $logDir "a9-$mode-model-health.json")
        Stop-AllTracked
    }
    $a9Ok = @($a9Results | Where-Object { !$_.passed }).Count -eq 0
    Set-Result 'A9' ($a9Ok ? 'PASS' : 'FAIL') 'embedded-mem + spawned-mem: review four prefixes 503 and model health 200/ok' `
        ($a9Results | ConvertTo-Json -Depth 6) '两种介质分别验证；前置配置失败不得记为通过'

    # ── A1 / A7：cargo 计数（本 runner 只记指引，不在这里跑）──────────────
    Set-Result 'A1' 'NOT RUN' 'cargo check --features http_api' '' '本 runner 只做实跑探测；cargo 计数由交付方单独跑并附尾行 + 耗时 + HEAD'
    Set-Result 'A7' 'NOT RUN' 'cargo test --features http_api --lib web_service::review' '' '同上'

} catch {
    Write-Host "`n[runner] 中断：$($_.Exception.Message)" -ForegroundColor Red
    Write-Host ($_.ScriptStackTrace) -ForegroundColor DarkGray
    Set-Result 'RUNNER' 'FAIL' '' '' $_.Exception.Message
} finally {
    Write-Step '收尾'
    Stop-AllTracked
    Start-Sleep -Milliseconds 500
    # 只报告，不 kill：这台机器上别的项目也在跑 aios-database。
    $left = @(Get-Process aios-database, surreal -ErrorAction SilentlyContinue)
    Write-Info "本脚本起的进程已全部收掉；机器上其余同名进程 $($left.Count) 个（不是本脚本起的，未动）"
    if (-not $KeepWorkDir) {
        $keep = Join-Path $script:RepoRoot ".tmp-acceptance-logs-$script:RunId"
        Copy-Item -Recurse -Force (Join-Path $script:WorkDir 'logs') $keep -ErrorAction SilentlyContinue
        $resolvedWork = [IO.Path]::GetFullPath($script:WorkDir)
        $tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
        if (!$resolvedWork.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or !(Split-Path $resolvedWork -Leaf).StartsWith('review-acceptance-')) { throw 'Unsafe cleanup path' }
        Remove-Item -LiteralPath $resolvedWork -Recurse -Force -ErrorAction SilentlyContinue
        Write-Info "数据目录已删；日志留在 $keep（不入库）"
    } else {
        Write-Info "工作目录保留：$script:WorkDir"
    }
    Pop-Location -ErrorAction SilentlyContinue

    Write-Step '汇总'
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    foreach ($k in $script:Results.Keys) {
        $r = $script:Results[$k]
        '{0,-8} {1,-8} {2}' -f $k, $r.verdict, $r.note | Write-Host
    }
    Write-Host "`n耗时 ${elapsed}s ；API $apiBase ；run $script:RunId"

    if ($JsonOut) {
        $payload = [ordered]@{
            runId = $script:RunId; head = $head; elapsedSec = $elapsed
            apiPort = $ApiPort; dbPort = $DbPort; legacyUp = $legacyUp
            dirtyFiles = $dirty; results = $script:Results
        }
        $payload | ConvertTo-Json -Depth 8 | Set-Content -Path $JsonOut -Encoding UTF8
        Write-Host "JSON: $JsonOut"
    }

    $failed = @($script:Results.Values | Where-Object { $_.verdict -eq 'FAIL' }).Count
    exit ($failed -gt 0 ? 1 : 0)
}
