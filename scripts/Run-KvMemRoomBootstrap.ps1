<#
.SYNOPSIS
  [已退役 2026-09-13] Bounded live gate for room-enabled read-through spawned-mem startup.

.DESCRIPTION
  **这条门跑不起来了，留档待改写。** 它建立在 `spawned-mem` 之上：服务自己拉一台有端口的
  memory SurrealDB，脚本再对着那个端口发 SQL（`Invoke-SurrealSql`）去挑正例 / 反例 refno。
  存储收成 mem / rocksdb 两档（ADR-074）之后，读透形态只剩 `mem`——库在服务进程里、没有
  端口，脚本发不了 SQL；而 `rocksdb` 档的数据面是 ingest，不是这条门要考的读透形态。

  要把它救回来，得把「挑正例 / 反例」那两条 SQL 换成走服务的 HTTP 面（例如按房间 / 面板
  的查询接口），其余流程（隔离目录、房间冷启动屏障、rebuild、冷重启两轮）都还成立。
  改写之前，它一跑就在下面这一行明确报错，而不是跑到一半对着连不上的端口超时。

  原始流程：Copies the backend and resources to a throwaway directory, starts it on
  isolated HTTP/Surreal ports, waits for the whole-MDB room bootstrap, verifies positive
  and negative room lookups, exercises memory rebuild, then repeats a cold restart.
#>
[CmdletBinding()]
param(
    [string]$BackendExe = 'D:\Rust\target\debug\aios-database.exe',
    [string]$SourceConfig = 'D:\work\plant-code\old\gen-model-refactor\DbOption.toml',
    [string]$SurrealExe = 'D:\work\plant-code\old\gen-model\bin\surreal.exe',
    [int]$HttpPort = 18084,
    [int]$StorePort = 18814,
    [int]$RebuildDbnum = 7997,
    [int]$TimeoutSec = 3600,
    [int]$Cycles = 2,
    [switch]$SkipRebuild,
    [string]$Output = "output/kv-mem-room-bootstrap/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
throw @"
Run-KvMemRoomBootstrap.ps1 已退役（2026-09-13，ADR-074）：它依赖服务自己拉起、带端口的 spawned-mem，
而存储只剩 mem（进程内、无端口）/ rocksdb（连配置指的那台）两档。读透形态（mem）的库连不上 SQL，
脚本里 Invoke-SurrealSql 那两条查询无处可发。要恢复这条门，先把正例 / 反例的挑选改成走服务 HTTP 面。
"@
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $Output
New-Item -ItemType Directory -Force $out | Out-Null
$runDir = Join-Path $env:TEMP ("kv-mem-room-bootstrap-{0}" -f [guid]::NewGuid().ToString('N'))
$runExe = Join-Path $runDir 'aios-database.exe'
$runConfig = Join-Path $runDir 'DbOption.toml'
$stdout = Join-Path $out 'backend.stdout.log'
$stderr = Join-Path $out 'backend.stderr.log'
$baseUrl = "http://127.0.0.1:$HttpPort"
$cyclesEvidence = [Collections.Generic.List[object]]::new()
$backend = $null

function Set-TomlScalar([string]$text, [string]$key, [string]$line) {
    $active = "(?m)^\s*$([regex]::Escape($key))\s*=.*$"
    if ([regex]::IsMatch($text, $active)) {
        return [regex]::Replace($text, $active, $line, 1)
    }
    $commented = "(?m)^\s*#\s*$([regex]::Escape($key))\s*=.*$"
    if ([regex]::IsMatch($text, $commented)) {
        return [regex]::Replace($text, $commented, $line, 1)
    }
    return $text.TrimEnd() + "`r`n$line`r`n"
}

function Get-BackendProcesses {
    return @(Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $runExe })
}

function Stop-OwnedProcesses {
    foreach ($item in @(Get-BackendProcesses)) {
        Stop-Process -Id $item.ProcessId -Force -ErrorAction SilentlyContinue
    }
    foreach ($item in @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -match ":$StorePort(?:\s|$)" })) {
        Stop-Process -Id $item.ProcessId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
}

function Start-Backend([int]$cycle) {
    foreach ($port in @($HttpPort, $StorePort)) {
        $listener = Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue
        if ($listener) {
            throw "isolated port $port is occupied by PID $($listener.OwningProcess)"
        }
    }
    $old = @{
        DB_OPTION_FILE = $env:DB_OPTION_FILE
        AIOS_STORE_MODE = $env:AIOS_STORE_MODE
        AIOS_STARTUP_AUTORUN = $env:AIOS_STARTUP_AUTORUN
        AIOS_ROOM_MEMBERSHIP = $env:AIOS_ROOM_MEMBERSHIP
        AIOS_SURREAL_EXE = $env:AIOS_SURREAL_EXE
        AIOS_OPEN_BROWSER = $env:AIOS_OPEN_BROWSER
        RUST_MIN_STACK = $env:RUST_MIN_STACK
    }
    try {
        $env:DB_OPTION_FILE = $runConfig
        $env:AIOS_STORE_MODE = 'spawned-mem'
        $env:AIOS_STARTUP_AUTORUN = '0'
        $env:AIOS_ROOM_MEMBERSHIP = '1'
        $env:AIOS_SURREAL_EXE = $SurrealExe
        $env:AIOS_OPEN_BROWSER = '0'
        $env:RUST_MIN_STACK = '134217728'
        $script:backend = Start-Process -FilePath $runExe -ArgumentList 'serve' -WorkingDirectory $runDir `
            -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    }
    finally {
        foreach ($name in $old.Keys) {
            if ($null -eq $old[$name]) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
            else { Set-Item "Env:$name" $old[$name] }
        }
    }
    Write-Host "[room-bootstrap] cycle=$cycle pid=$($script:backend.Id)"
}

function Invoke-SurrealSql([string]$sql) {
    $headers = @{
        Accept = 'application/json'
        'surreal-ns' = '1516'
        'surreal-db' = 'AvevaMarineSample'
        Authorization = 'Basic ' + [Convert]::ToBase64String(
            [Text.Encoding]::ASCII.GetBytes('root:root')
        )
    }
    return @(Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$StorePort/sql" `
        -Headers $headers -ContentType 'application/surrealql' -Body $sql -TimeoutSec 120)
}

function Convert-PeIdToRefno([string]$value) {
    $id = $value -replace '^pe:', '' -replace '[⟨⟩]', ''
    return $id -replace '_', '/'
}

function Invoke-RoomLookup([string]$refno) {
    $body = @{ tool = 'e3d.room.lookup'; arguments = @{ refno = $refno } } |
        ConvertTo-Json -Depth 5 -Compress
    return (Invoke-RestMethod -Method Post -Uri "$baseUrl/api/v1/query" `
        -ContentType 'application/json' -Body $body -TimeoutSec 120).result
}

function Test-CurrentRoomResults {
    $positiveRows = @((Invoke-SurrealSql @'
SELECT <string>in AS panel, <string>out AS member, room_num
FROM room_relate ORDER BY room_num, in, out LIMIT 1;
'@)[0].result)
    if ($positiveRows.Count -ne 1) { throw 'room bootstrap produced no positive room_relate edge' }
    $positiveRefno = Convert-PeIdToRefno ([string]$positiveRows[0].member)
    $positive = Invoke-RoomLookup $positiveRefno
    if ($positive.calculation_status -ne 'ready_nonempty' -or @($positive.memberships).Count -eq 0) {
        throw "positive lookup is not ready_nonempty: $($positive | ConvertTo-Json -Depth 8 -Compress)"
    }
    $negativeRows = @((Invoke-SurrealSql @'
SELECT VALUE <string>in FROM inst_relate
WHERE in NOT IN (SELECT VALUE out FROM room_relate)
ORDER BY in LIMIT 1;
'@)[0].result)
    if ($negativeRows.Count -ne 1) { throw 'room bootstrap produced no explicit outside candidate' }
    $negativeRefno = Convert-PeIdToRefno ([string]$negativeRows[0])
    $negative = Invoke-RoomLookup $negativeRefno
    if ($negative.calculation_status -ne 'ready_empty' -or @($negative.memberships).Count -ne 0) {
        throw "negative lookup is not ready_empty: $($negative | ConvertTo-Json -Depth 8 -Compress)"
    }
    return [pscustomobject]@{
        positive_refno = $positiveRefno
        positive_room_refno = $positive.memberships[0].room_refno
        positive_panel_refno = $positive.memberships[0].panel_refno
        positive_room_num = $positive.memberships[0].room_num
        negative_refno = $negativeRefno
        positive = $positive
        negative = $negative
    }
}

function Wait-Task([string]$taskId) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $task = Invoke-RestMethod -Uri "$baseUrl/api/v1/tasks/$([uri]::EscapeDataString($taskId))" `
            -TimeoutSec 30
        if ($task.state -in @('succeeded', 'partial', 'failed', 'yielded')) { return $task }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    throw "task $taskId did not finish within $TimeoutSec seconds"
}

function Invoke-MemoryRebuild {
    $response = Invoke-RestMethod -Method Post `
        -Uri "$baseUrl/api/v1/dbnums/$RebuildDbnum/model/rebuild" `
        -ContentType 'application/json' -Body '{}' -TimeoutSec 180
    if ($response.state -eq 'queued') {
        throw "memory rebuild entered the durable queue: $($response | ConvertTo-Json -Compress)"
    }
    $task = Wait-Task ([string]$response.task_id)
    if ($task.state -ne 'succeeded') {
        throw "memory rebuild did not succeed: $($task | ConvertTo-Json -Depth 8 -Compress)"
    }
    return $task
}

function Wait-RoomBootstrap([int]$cycle) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $last = ''
    $sawBarrier = $false
    do {
        if ((Get-BackendProcesses).Count -eq 0) {
            $errorText = if (Test-Path $stderr) { Get-Content $stderr -Raw } else { '' }
            throw "backend exited during room bootstrap: $errorText"
        }
        try {
            $health = Invoke-RestMethod -Uri "$baseUrl/api/v1/health" -TimeoutSec 10
            $bootstrap = $health.initialization.room_bootstrap
            $summary = "status=$($bootstrap.status) dbnums=$($bootstrap.dbnums_done)/$($bootstrap.dbnums_total) roots=$($bootstrap.roots_ready)/$($bootstrap.roots_total) failed=$($bootstrap.roots_failed) current=$($bootstrap.current_dbnum) rss=$($bootstrap.rss_mib)MiB"
            if ($summary -ne $last) {
                Write-Host "[room-bootstrap] cycle=$cycle $summary"
                $last = $summary
            }
            if ($bootstrap.status -ne 'ready') {
                $sawBarrier = $true
                if ($health.initialization.model_ready -eq $true -or
                    $health.features.room.status -eq 'available') {
                    throw "readiness escaped the room barrier: $summary"
                }
            }
            if ($bootstrap.status -in @('partial', 'blocked')) {
                throw "room bootstrap $($bootstrap.status): $($bootstrap | ConvertTo-Json -Depth 8 -Compress)"
            }
            if ($bootstrap.status -eq 'ready') {
                if (-not $sawBarrier) { throw 'health never exposed the initializing barrier' }
                if ($health.initialization.model_ready -ne $true -or
                    $health.features.room.status -ne 'available') {
                    throw "ready bootstrap did not release model/room readiness"
                }
                if ($health.data_face -ne 'read-through' -or
                    $health.sul_db.medium -ne 'spawned-mem' -or
                    $health.sul_db.durable -ne $false) {
                    throw "store contract mismatch: $($health | ConvertTo-Json -Depth 5 -Compress)"
                }
                return $health
            }
        }
        catch {
            if ($null -eq $_.Exception.Response) {
                Start-Sleep -Seconds 2
                continue
            }
            throw
        }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    throw "room bootstrap did not finish within $TimeoutSec seconds"
}

foreach ($required in @($BackendExe, $SourceConfig, $SurrealExe)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "missing live prerequisite: $required"
    }
}
New-Item -ItemType Directory -Force $runDir | Out-Null
Copy-Item -LiteralPath $BackendExe -Destination $runExe
foreach ($name in @('resource', 'web')) {
    $source = Join-Path $repo $name
    if (Test-Path -LiteralPath $source) {
        Copy-Item -LiteralPath $source -Destination (Join-Path $runDir $name) -Recurse
    }
}
$config = Get-Content -LiteralPath $SourceConfig -Raw
$config = Set-TomlScalar $config 'v_ip' 'v_ip = "127.0.0.1"'
$config = Set-TomlScalar $config 'v_port' "v_port = $StorePort"
$config = Set-TomlScalar $config 'store_mode' 'store_mode = "spawned-mem"'
$config = Set-TomlScalar $config 'http_api_addr' "http_api_addr = `"127.0.0.1:$HttpPort`""
$config = Set-TomlScalar $config 'http_api_cors' 'http_api_cors = ["*"]'
$config = Set-TomlScalar $config 'startup_autorun' 'startup_autorun = false'
$config = Set-TomlScalar $config 'room_membership' 'room_membership = true'
[IO.File]::WriteAllText($runConfig, $config, [Text.UTF8Encoding]::new($false))

$succeeded = $false
try {
    foreach ($cycle in 1..$Cycles) {
        Start-Backend $cycle
        $health = Wait-RoomBootstrap $cycle
        $lookups = Test-CurrentRoomResults
        $rebuild = $null
        if ($cycle -eq 1 -and -not $SkipRebuild) {
            $rebuild = Invoke-MemoryRebuild
        }
        $cyclesEvidence.Add([pscustomobject]@{
            cycle = $cycle
            started_at = $health.started_at
            bootstrap = $health.initialization.room_bootstrap
            room_feature = $health.features.room
            lookups = $lookups
            rebuild = $rebuild
        })
        Stop-OwnedProcesses
    }
    [pscustomobject]@{
        generated_at = (Get-Date).ToString('o')
        backend_exe = $BackendExe
        http_port = $HttpPort
        store_port = $StorePort
        cycles = $cyclesEvidence
        stdout = $stdout
        stderr = $stderr
    } | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath (Join-Path $out 'evidence.json') -Encoding utf8
    $succeeded = $true
}
finally {
    Stop-OwnedProcesses
    if ($succeeded) {
        Remove-Item -LiteralPath $runDir -Recurse -Force -ErrorAction SilentlyContinue
    } else {
        Write-Warning "live gate failed; retained runtime at $runDir"
    }
}

Write-Host "kv-mem room bootstrap live gate PASS: $out"
