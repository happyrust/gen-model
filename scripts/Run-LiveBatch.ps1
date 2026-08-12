<#
.SYNOPSIS
    按批次清单逐项跑 live（#[ignore]）用例，产出结构化报告供台账回填。

.DESCRIPTION
    台账：docs/2026-08-12_live-test-ledger.md（7-27 计划 Gate 3 的执行载体）。

    设计取舍：
      * 每个用例一个独立 cargo test 进程——live 用例共享全局连接与进程态，
        同进程串跑会互相污染；进程隔离让"谁红"可归因。
      * 过滤用**函数名子串**而非 --exact 全路径（模块路径记不住也不稳定），
        但要求本次恰好命中 1 个测试，命中 0/多个按"清单错误"记红。
      * 先做前置检查：配置文件在、目标 Surreal 端口在听、testbed 项目副本锁
        空闲——批跑写的是沙箱，但沙箱同一时刻只能有一个写者。

.EXAMPLE
    powershell -File scripts\Run-LiveBatch.ps1 -Manifest scripts\live-batches\batch1-selfcontained.json
    powershell -File scripts\Run-LiveBatch.ps1 -Manifest ... -Only live_room_fixture_parity
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Manifest,
    # 只跑清单中匹配这些子串的条目（调试/重跑单项用）。
    # 注意 `powershell -File` 不做数组绑定，"a,b,c" 会整串进来——下面自行拆逗号。
    [string[]]$Only = @(),
    [string]$Output = "output/live-batch/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    $spec = Get-Content -LiteralPath (Join-Path $repo $Manifest) -Raw | ConvertFrom-Json
    $out = Join-Path $repo $Output
    New-Item -ItemType Directory -Force $out | Out-Null

    # ── 前置检查 ─────────────────────────────────────────────────────────────
    $configPath = Join-Path $repo "$($spec.config).toml"
    if (-not (Test-Path -LiteralPath $configPath)) { throw "定靶配置不存在: $configPath" }
    $configText = Get-Content -LiteralPath $configPath -Raw
    if ($configText -notmatch 'v_port\s*=\s*(\d+)') { throw "配置里读不到 v_port: $configPath" }
    $port = [int]$Matches[1]
    $probe = New-Object Net.Sockets.TcpClient
    try {
        $probe.Connect('127.0.0.1', $port)
    } catch {
        throw "目标 SurrealDB 127.0.0.1:$port 不在听——先起对应实例（testbed: Start-TestSurreal.ps1）"
    } finally { $probe.Dispose() }
    # testbed 项目副本单实例锁：有人（run_full_loop / pytest 房间档 / 服务）在用
    # 副本时不批跑，避免两个写者互踩。锁文件存在即视为占用（进程死了会留渣，
    # 但"宁可误停，不可互踩"）。
    if ($configText -match 'project_path\s*=\s*"([^"]+)"') {
        $projectsRoot = $Matches[1]
        $projectName = if ($configText -match 'project_name\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }
        # 锁在项目**子目录**下（full_init 的单实例锁协议），不在 projects 根。
        $lock = Join-Path (Join-Path $projectsRoot $projectName) '.gen-model.instance.lock'
        if (Test-Path -LiteralPath $lock) {
            $owner = [IO.File]::ReadAllText($lock)
            if ($owner -match 'pid=(\d+)' -and (Get-Process -Id ([int]$Matches[1]) -ErrorAction SilentlyContinue)) {
                throw "项目副本锁属主存活: $lock ——testbed 正被别的进程使用"
            }
            # 属主已死的残留锁：full_init 自己也会覆盖，这里只提示不拦。
            Write-Host "发现残留项目锁（属主已死），full_init 会接管: $lock"
        }
    }

    $env:DB_OPTION_FILE = $spec.config
    $env:RUST_MIN_STACK = '134217728'
    # 一部分 live 用例（room_fixture / aabb_tree 系）不读 DB_OPTION_FILE，走
    # AIOS_LIVE_WS/NS/DB 三件套连库——从同一份配置推导，保证两套寻址指向同一个靶。
    $env:AIOS_LIVE_WS = "ws://127.0.0.1:$port"
    if ($configText -match 'surreal_ns\s*=\s*(\d+)') { $env:AIOS_LIVE_NS = $Matches[1] }
    if ($configText -match 'project_name\s*=\s*"([^"]+)"') { $env:AIOS_LIVE_DB = $Matches[1] }
    Write-Host "定靶 $($spec.config)（:$port，AIOS_LIVE_WS=$($env:AIOS_LIVE_WS)），报告目录 $out"

    # 先把测试二进制建好，免得第一项的耗时里混进整仓编译。经 cmd 包一层：
    # cargo 往 stderr 写进度（如等构建锁），PowerShell 5.1 在 Stop 偏好下会把
    # 原生命令的 stderr 行当错误记录直接中止脚本。
    & cmd /c "cargo build --lib --tests --features $($spec.features) 2>&1" | Select-Object -Last 1
    if ($LASTEXITCODE) { throw "cargo build --tests failed ($LASTEXITCODE)" }

    # ── 逐项执行 ─────────────────────────────────────────────────────────────
    $results = New-Object System.Collections.Generic.List[object]
    $entries = @($spec.tests | Where-Object {
        if (-not $Only) { return $true }
        foreach ($needle in $Only) { if ($_.name -like "*$needle*") { return $true } }
        return $false
    })
    $index = 0
    foreach ($entry in $entries) {
        $index++
        $name = $entry.name
        $timeout = if ($entry.timeout_secs) { [int]$entry.timeout_secs } else { [int]$spec.default_timeout_secs }
        # 条目级环境变量（如 AIOS_EXPECT_DESI_COUNT / AIOS_GEOM_COVERAGE_ROOTS）：
        # 跑前设、跑完清，避免串到下一条。
        $entryEnv = @{}
        if ($entry.env) {
            foreach ($prop in $entry.env.PSObject.Properties) { $entryEnv[$prop.Name] = [string]$prop.Value }
        }
        foreach ($key in $entryEnv.Keys) { Set-Item -Path "Env:$key" -Value $entryEnv[$key] }
        $log = Join-Path $out ("{0:d2}-{1}.log" -f $index, $name)
        Write-Host ("[{0}/{1}] {2}" -f $index, $entries.Count, $name) -NoNewline

        $started = Get-Date
        # Start-Process + 超时：卡死的 live 用例不能拖死整批。
        $process = Start-Process -FilePath 'cargo' -ArgumentList @(
            'test', '--lib', '--features', $spec.features, $name,
            '--', '--ignored', '--nocapture'
        ) -WorkingDirectory $repo -RedirectStandardOutput $log -RedirectStandardError "$log.err" `
          -NoNewWindow -PassThru
        $finished = $process.WaitForExit($timeout * 1000)
        if (-not $finished) {
            & taskkill.exe /PID $process.Id /T /F 2>&1 | Out-Null
            $status = 'timeout'
        }
        $seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)

        $body = (Get-Content -LiteralPath $log -Raw -ErrorAction SilentlyContinue) + "`n" +
                (Get-Content -LiteralPath "$log.err" -Raw -ErrorAction SilentlyContinue)
        if ($finished) {
            # 恰好 1 个命中才算数：0 个 = 名字打错，多个 = 子串撞名，都得修清单。
            $ran = if ($body -match 'running (\d+) test') { [int]$Matches[1] } else { -1 }
            if ($ran -ne 1) {
                $status = "ambiguous($ran matched)"
            } elseif ($body -match 'test result: ok\. 1 passed') {
                $status = 'pass'
            } else {
                $status = 'fail'
            }
        }
        $tail = if ($status -in @('pass')) { '' } else {
            (($body -split "`n") | Select-Object -Last 25) -join "`n"
        }
        $results.Add([pscustomobject]@{
            name = $name; status = $status; seconds = $seconds; log = $log; tail = $tail
        })
        Write-Host ("  {0}（{1}s）" -f $status, $seconds)
        foreach ($key in $entryEnv.Keys) { Remove-Item -Path "Env:$key" -ErrorAction SilentlyContinue }
    }

    # ── 汇总 ─────────────────────────────────────────────────────────────────
    $report = [pscustomobject]@{
        manifest  = $Manifest
        config    = $spec.config
        started   = (Get-Date).ToString('s')
        commit    = (git rev-parse --short HEAD)
        results   = $results
    }
    $reportPath = Join-Path $out 'report.json'
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    $passed = @($results | Where-Object status -eq 'pass').Count
    Write-Host "`n$passed/$($results.Count) pass；报告 $reportPath"
    foreach ($r in $results | Where-Object status -ne 'pass') {
        Write-Host ("  [{0}] {1}" -f $r.status, $r.name)
    }
    if ($passed -ne $results.Count) { exit 1 }
}
finally {
    Pop-Location
}
