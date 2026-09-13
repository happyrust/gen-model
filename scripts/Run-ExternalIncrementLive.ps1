<#
.SYNOPSIS
    external（ingest）形态稳态增量的 live 操作单：隔离运行目录 + 隔离 rocksdb + 隔离端口，
    按阶段走「准备 → 起库 / 起服务 → 等基线 → 每次 SAVEWORK 打点 → 盯到一致 → 出账」。
    计划：docs/plans/2026-09-08-external-increment-refactor-plan.md §4 S0 / S1、§5 验收门。

.DESCRIPTION
    这份脚本**不替你起长驻进程**——surreal 与 aios-database 都在你自己的终端里跑（`serve` 阶段
    就是在当前控制台前台跑服务）。脚本负责的是：把配置派生出来、把判据写死、把每一次
    SAVEWORK 的时刻与 /dbnums 的变化记成账，免得凭记忆说「大概一分钟就好了」。

    判据（计划 §5，live #1 / #2 共同）：
      1. 每次 SAVEWORK 起 ≤ 120 s，/api/v1/dbnums 里该库 model_verdict = in_sync
         （applied_sesno == model_sesno == file_latest_sesno，且 model_chasing_roots = 0）；
      2. 变更元素的 pe / <NOUN> / ATT_UDA / pe_owner 行与 stream_baseline 逐字段相同——**不在本脚本里**，
         由会话读库判定（需要 e3d-io 侧渲染，脚本只留下 SAVEWORK 的会话号与时刻）；
      3. OWNER 搬迁两端根都重生成；4. FRMW 改名后 room_num 换新且房间边逐边 == 全量重建——同样由会话判定；
      5. 全程无人工 POST /update/execute：`watch` 阶段每一拍都核 /health 的 manual 计数，动了就记红。

    阶段：
      prepare        派生配置到 output/external-live/<stamp>/，写 current.json，打印后续命令
      serve          在当前控制台前台起 aios-database（读 current.json 的配置与端口）
      baseline-wait  等 7997 / 7999 基线：applied == file_latest 且 in_sync；快照 /dbnums、/health
      arm            （可选，L2-1）在 E3D 动手**之前**跑：把变更根/对照根/元素/树/搜索的
                     HTTP 读面基线抓进 marks/<label>-http-capture.json（Http-IncrementAssert.ps1）
      mark           在 E3D 里 SAVEWORK 之后**立刻**跑：记下时刻与当时的 /dbnums
      watch          从 mark 起盯该库到 in_sync 或超时；armed 的 label 收敛后自动做
                     HTTP 读面对拍（变更根必须变、对照根逐字不变、无新失败），记入 ledger.md
      report         打印 ledger

.PARAMETER Phase
    见上。

.PARAMETER Label
    arm / mark / watch 用：这一次 SAVEWORK 的名字（如 bran-modify、owner-move、frmw-rename、equi-scenario-3）。

.PARAMETER Dbnum
    watch 用：这一次 SAVEWORK 动的是哪个库（7997 / 7999）。

.PARAMETER ChangedRoots / DeletedRoots / ControlRoots / Elements / TreeParents / SearchQueries
    arm 用（L2-1 HTTP 断言的对象清单，refno 一律 "a/b"）：
      -ChangedRoots   这次要动的生成根：收敛后 records 必须变化
      -DeletedRoots   这次要删的生成根：收敛后 records 必须为空
      -ControlRoots   不动的对照根：收敛后 records 必须逐字不变（不重算证据）
      -Elements       被改元素：收敛后 element/attributes 必须变化
      -TreeParents    要留树切片证据的父节点（tree/children 前后对比，只报不判）
      -SearchQueries  要留搜索切片证据的 NAME 子串（/search 前后对比，只报不判）

.PARAMETER SurrealPort / HttpPort
    prepare 用：隔离端口。默认 18109 / 18122，与 8009 / 8010 / 8022 那几台都不挨着。

.PARAMETER Exe
    serve 用：aios-database 二进制。默认 target/release/aios-database.exe（相对仓根）。

.EXAMPLE
    # 终端 A（一次）
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase prepare
    ./scripts/Start-Surreal8009.ps1 -Bind 127.0.0.1:18109 -Datastore rocksdb:output/external-live/<stamp>/.surreal/external-live
    # 终端 B
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase serve
    # 终端 C
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase baseline-wait
    # ……动手前布防（可选，L2-1 HTTP 断言）：
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase arm -Label bran-modify `
        -ChangedRoots 24381/100817 -ControlRoots 24381/100677 -Elements 24381/100823
    # ……在 E3D 里改一根 BRAN → SAVEWORK，立刻：
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase mark -Label bran-modify
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase watch -Label bran-modify -Dbnum 7997
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('prepare', 'serve', 'baseline-wait', 'arm', 'mark', 'watch', 'report')]
    [string]$Phase,
    [string]$Label,
    [int]$Dbnum,
    [int]$SurrealPort = 18109,
    [int]$HttpPort = 18122,
    [string]$Exe,
    [string]$BaseConfig = 'db_options/DbOption-init-7997.toml',
    [int[]]$WatchDbnums = @(7997, 7999),
    [int]$InSyncBudgetSecs = 120,
    [int]$BaselineTimeoutMins = 240,
    # ── arm 用：L2-1 HTTP 断言对象清单（含义见 .PARAMETER 注释）──
    [string[]]$ChangedRoots = @(),
    [string[]]$DeletedRoots = @(),
    [string[]]$ControlRoots = @(),
    [string[]]$Elements = @(),
    [string[]]$TreeParents = @(),
    [string[]]$SearchQueries = @()
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$liveRoot = Join-Path $repo 'output\external-live'
$currentPath = Join-Path $liveRoot 'current.json'

function Read-Current {
    if (-not (Test-Path -LiteralPath $currentPath)) {
        throw "没有 $currentPath：先跑 -Phase prepare"
    }
    Get-Content -LiteralPath $currentPath -Raw | ConvertFrom-Json
}

function Get-Dbnums {
    param([int]$Port)
    # 无查询参数的 /dbnums 是启动骨架摘要，不包含持久水位。live 验收必须
    # 显式请求当前 AMS /ALL 身份，否则 baseline-wait 会永远看不到 applied/model。
    Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/dbnums?project=AvevaMarineSample&mdb=ALL&namespace=1516" -TimeoutSec 30
}

function Get-Health {
    param([int]$Port)
    Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/health" -TimeoutSec 10
}

function Select-Row {
    param($Dbnums, [int]$Number)
    $Dbnums.dbnums | Where-Object { [int]$_.dbnum -eq $Number } | Select-Object -First 1
}

function Format-Row {
    param($Row)
    if (-not $Row) { return '（/dbnums 里没有这一行）' }
    'dbnum={0} applied={1} model={2} file={3} verdict={4}{5} chasing={6} dead={7}' -f `
        $Row.dbnum, $Row.applied_sesno, $Row.model_sesno, $Row.file_latest_sesno,
        $Row.model_verdict, $(if ($Row.model_verdict_reason) { "($($Row.model_verdict_reason))" } else { '' }),
        $Row.model_chasing_roots, $Row.model_dead_roots
}

function Get-ManualExecuteCount {
    param($Health)
    # /health 的手动执行计数不同版本落在不同格子里；找不到就回 $null，账上记「未观察」而不是 0。
    foreach ($path in @('manual_update.executed', 'queue.manual_executed', 'manual_executed')) {
        $node = $Health
        $ok = $true
        foreach ($segment in $path.Split('.')) {
            if ($null -eq $node -or -not ($node.PSObject.Properties.Name -contains $segment)) { $ok = $false; break }
            $node = $node.$segment
        }
        if ($ok -and $null -ne $node) { return [int]$node }
    }
    return $null
}

switch ($Phase) {
    'prepare' {
        $stamp = Get-Date -Format yyyyMMdd-HHmmss
        $run = Join-Path $liveRoot $stamp
        New-Item -ItemType Directory -Force (Join-Path $run 'marks') | Out-Null
        New-Item -ItemType Directory -Force (Join-Path $run '.surreal') | Out-Null

        $base = Join-Path $repo $BaseConfig
        if (-not (Test-Path -LiteralPath $base)) { throw "基础配置不存在：$base" }
        $text = Get-Content -LiteralPath $base -Raw

        # 只改这几格，其余与 7997 初始化夹具保持一致。external 的数据面由
        # store_mode 派生，派生配置不保留旧的 data_face 声明。
        $text = [regex]::Replace($text, '(?m)^v_port\s*=\s*\d+', "v_port = $SurrealPort")
        $text = [regex]::Replace($text, '(?m)^kv_port\s*=\s*"\d+"', "kv_port = `"$($SurrealPort + 1)`"")
        $text = [regex]::Replace($text, '(?m)^#\s*http_api_addr\s*=.*$', "http_api_addr = `"127.0.0.1:$HttpPort`"")
        if ($text -notmatch '(?m)^http_api_addr\s*=') {
            $text += "`nhttp_api_addr = `"127.0.0.1:$HttpPort`"`n"
        }
        $watchList = ($WatchDbnums | ForEach-Object { $_ }) -join ', '
        $text = [regex]::Replace($text, '(?m)^manual_db_nums\s*=.*$', "manual_db_nums = [$watchList]")
        if ($text -notmatch '(?m)^store_mode\s*=') {
            throw "基础配置缺 store_mode，护栏本该拦住它：$base"
        }
        $text = [regex]::Replace($text, '(?m)^data_face\s*=.*(?:\r?\n)?', '')
        $text += @"

# ── external 稳态增量 live（scripts/Run-ExternalIncrementLive.ps1 派生，$stamp）──
# 计划 docs/plans/2026-09-08-external-increment-refactor-plan.md §3 Q7 / Q8 / Q10：
# 监听限定到两库、启动自动干活、常驻房间模型关着验今天的路。
watch_dbnums = [$watchList]
startup_autorun = true
room_resident_model = false
"@
        $configPath = Join-Path $run 'DbOption-external-live.toml'
        Set-Content -LiteralPath $configPath -Value $text -Encoding UTF8

        $datastore = "rocksdb:" + (Join-Path $run '.surreal\external-live')
        $current = [ordered]@{
            stamp          = $stamp
            run_dir        = $run
            config         = $configPath
            config_env     = ($configPath -replace '\.toml$', '')
            surreal_port   = $SurrealPort
            http_port      = $HttpPort
            datastore      = $datastore
            watch_dbnums   = $WatchDbnums
            in_sync_budget = $InSyncBudgetSecs
        }
        New-Item -ItemType Directory -Force $liveRoot | Out-Null
        $current | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $currentPath -Encoding UTF8
        Set-Content -LiteralPath (Join-Path $run 'ledger.md') -Encoding UTF8 -Value @"
# external 稳态增量 live · $stamp

| label | dbnum | SAVEWORK 时刻 | 起点 applied/model/file | 一致时刻 | 用时 s | 预算 s | 结果 | 终点 applied/model/file | 手动执行 | 备注 |
|---|---|---|---|---|---|---|---|---|---|---|
"@

        Write-Host "运行目录 : $run"
        Write-Host "配置     : $configPath"
        Write-Host "SurrealDB: 127.0.0.1:$SurrealPort  ($datastore)"
        Write-Host "HTTP     : 127.0.0.1:$HttpPort"
        Write-Host ''
        Write-Host '下一步（各自一个终端）：' -ForegroundColor Cyan
        Write-Host "  A) ./scripts/Start-Surreal8009.ps1 -Bind 127.0.0.1:$SurrealPort -Datastore `"$datastore`""
        Write-Host "  B) ./scripts/Run-ExternalIncrementLive.ps1 -Phase serve"
        Write-Host "  C) ./scripts/Run-ExternalIncrementLive.ps1 -Phase baseline-wait"
    }

    'serve' {
        $current = Read-Current
        if (-not $Exe) { $Exe = Join-Path $repo 'target\release\aios-database.exe' }
        if (-not (Test-Path -LiteralPath $Exe)) { throw "找不到二进制：$Exe（先 cargo build --release，或 -Exe 指定）" }
        $listener = Get-NetTCPConnection -LocalPort $current.http_port -State Listen -ErrorAction SilentlyContinue
        if ($listener) { throw "HTTP 端口 $($current.http_port) 已被 pid $($listener.OwningProcess) 占着，不硬上" }
        $surreal = Get-NetTCPConnection -LocalPort $current.surreal_port -State Listen -ErrorAction SilentlyContinue
        if (-not $surreal) { throw "SurrealDB 端口 $($current.surreal_port) 上没人监听：先跑终端 A 那条 Start-Surreal8009.ps1" }

        # 介质形态显式为 external；数据面由 store_mode 自动派生。
        $env:DB_OPTION_FILE = $current.config_env
        $env:AIOS_STORE_MODE = 'external'
        Remove-Item Env:AIOS_DATA_FACE -ErrorAction SilentlyContinue
        $env:RUST_MIN_STACK = '134217728'
        $env:RUST_BACKTRACE = '1'
        $log = Join-Path $current.run_dir 'serve.log'
        Write-Host "配置 : $($current.config)"
        Write-Host "日志 : $log（同时打在这个控制台）"
        Write-Host '前台运行，Ctrl+C 退出。' -ForegroundColor Yellow
        Push-Location $repo
        try {
            & $Exe 2>&1 | Tee-Object -FilePath $log -Append
        }
        finally { Pop-Location }
    }

    'baseline-wait' {
        $current = Read-Current
        $deadline = (Get-Date).AddMinutes($BaselineTimeoutMins)
        $lastLine = ''
        while ((Get-Date) -lt $deadline) {
            try {
                $dbnums = Get-Dbnums -Port $current.http_port
                $health = Get-Health -Port $current.http_port
            }
            catch {
                Write-Host ("{0} 服务还没答话：{1}" -f (Get-Date -Format HH:mm:ss), $_.Exception.Message)
                Start-Sleep -Seconds 10
                continue
            }
            $rows = @($current.watch_dbnums | ForEach-Object { Select-Row -Dbnums $dbnums -Number $_ })
            $line = ($rows | ForEach-Object { Format-Row $_ }) -join ' | '
            $line += ' | init=' + $health.initialization.status + ' data_ready=' + $health.initialization.data_ready + ' model_ready=' + $health.initialization.model_ready
            if ($line -ne $lastLine) { Write-Host ("{0} {1}" -f (Get-Date -Format HH:mm:ss), $line); $lastLine = $line }
            $done = ($rows.Count -eq $current.watch_dbnums.Count) -and -not ($rows | Where-Object {
                    -not $_ -or [int]$_.applied_sesno -le 0 -or [int]$_.applied_sesno -ne [int]$_.file_latest_sesno -or $_.model_verdict -ne 'in_sync'
                })
            if ($done -and $health.initialization.data_ready -and $health.initialization.model_ready) {
                $dbnums | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $current.run_dir 'baseline-dbnums.json') -Encoding UTF8
                $health | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $current.run_dir 'baseline-health.json') -Encoding UTF8
                Write-Host '基线到位：两库 applied == file_latest 且 in_sync，data_ready / model_ready 都为真。' -ForegroundColor Green
                Write-Host '现在可以在 E3D 里做第一条变化；SAVEWORK 之后立刻跑 -Phase mark -Label <名字>。' -ForegroundColor Cyan
                exit 0
            }
            Start-Sleep -Seconds 30
        }
        throw "基线在 $BaselineTimeoutMins 分钟内没到位；看 serve.log 与 /health.initialization.blockers"
    }

    'arm' {
        # L2-1：SAVEWORK 之前把 HTTP 读面基线抓下来（改动前的 records / attributes / 树 / 搜索 /
        # 失败账），watch 收敛后自动对拍。必须先于 E3D 动手——mark 是 SAVEWORK 之后的时刻，
        # 那时服务可能已经开始吸收这次会话，基线就脏了。
        if (-not $Label) { throw 'arm 需要 -Label' }
        $null = Read-Current   # 只为确认 prepare 已跑、current.json 在位
        $assertArgs = @{}
        if ($ChangedRoots) { $assertArgs.ChangedRoots = $ChangedRoots }
        if ($DeletedRoots) { $assertArgs.DeletedRoots = $DeletedRoots }
        if ($ControlRoots) { $assertArgs.ControlRoots = $ControlRoots }
        if ($Elements) { $assertArgs.Elements = $Elements }
        if ($TreeParents) { $assertArgs.TreeParents = $TreeParents }
        if ($SearchQueries) { $assertArgs.SearchQueries = $SearchQueries }
        if ($assertArgs.Count -eq 0) {
            throw 'arm 至少要给一组对象：-ChangedRoots / -DeletedRoots / -ControlRoots / -Elements / -TreeParents / -SearchQueries'
        }
        & (Join-Path $PSScriptRoot 'Http-IncrementAssert.ps1') -Phase capture -Label $Label @assertArgs
        if ($LASTEXITCODE -ne 0) { throw "HTTP 基线抓取失败（退出码 $LASTEXITCODE），没有基线就没有对拍" }
        Write-Host "已布防 $Label：现在去 E3D 做变更并 SAVEWORK，然后立刻 -Phase mark -Label $Label" -ForegroundColor Cyan
    }

    'mark' {
        if (-not $Label) { throw 'mark 需要 -Label' }
        $current = Read-Current
        $at = Get-Date
        $dbnums = Get-Dbnums -Port $current.http_port
        $health = Get-Health -Port $current.http_port
        $mark = [ordered]@{
            label          = $Label
            marked_at      = $at.ToString('o')
            manual_execute = Get-ManualExecuteCount $health
            rows           = @($current.watch_dbnums | ForEach-Object { Select-Row -Dbnums $dbnums -Number $_ })
        }
        $mark | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $current.run_dir "marks\$Label.json") -Encoding UTF8
        Write-Host ("已打点 {0} @ {1}" -f $Label, $at.ToString('HH:mm:ss'))
        $mark.rows | ForEach-Object { Write-Host ('  ' + (Format-Row $_)) }
        Write-Host "接着：./scripts/Run-ExternalIncrementLive.ps1 -Phase watch -Label $Label -Dbnum <7997|7999>" -ForegroundColor Cyan
    }

    'watch' {
        if (-not $Label) { throw 'watch 需要 -Label' }
        if (-not $Dbnum) { throw 'watch 需要 -Dbnum' }
        $current = Read-Current
        $markPath = Join-Path $current.run_dir "marks\$Label.json"
        if (-not (Test-Path -LiteralPath $markPath)) { throw "没有打点 $markPath：先跑 -Phase mark -Label $Label" }
        $mark = Get-Content -LiteralPath $markPath -Raw | ConvertFrom-Json
        $markedAt = [datetime]::Parse($mark.marked_at)
        $start = $mark.rows | Where-Object { [int]$_.dbnum -eq $Dbnum } | Select-Object -First 1
        if (-not $start) { throw "打点里没有 dbnum=$Dbnum 那一行" }
        $startText = '{0}/{1}/{2}' -f $start.applied_sesno, $start.model_sesno, $start.file_latest_sesno
        Write-Host ("盯 dbnum={0}，起点 applied/model/file = {1}，预算 {2} s（从 {3} 起算）" -f $Dbnum, $startText, $current.in_sync_budget, $markedAt.ToString('HH:mm:ss'))

        $hardStop = $markedAt.AddSeconds([Math]::Max(600, $current.in_sync_budget * 5))
        $lastLine = ''
        $manualMoved = $false
        $result = 'TIMEOUT'
        $row = $null
        while ((Get-Date) -lt $hardStop) {
            try {
                $dbnums = Get-Dbnums -Port $current.http_port
                $health = Get-Health -Port $current.http_port
            }
            catch {
                Write-Host ("{0} 服务没答话：{1}" -f (Get-Date -Format HH:mm:ss), $_.Exception.Message)
                Start-Sleep -Seconds 5
                continue
            }
            $row = Select-Row -Dbnums $dbnums -Number $Dbnum
            $manualNow = Get-ManualExecuteCount $health
            if ($null -ne $manualNow -and $null -ne $mark.manual_execute -and $manualNow -ne $mark.manual_execute) { $manualMoved = $true }
            $line = Format-Row $row
            if ($line -ne $lastLine) { Write-Host ("{0} +{1,4}s {2}" -f (Get-Date -Format HH:mm:ss), [int]((Get-Date) - $markedAt).TotalSeconds, $line); $lastLine = $line }
            $advanced = $row -and [int]$row.applied_sesno -gt [int]$start.applied_sesno
            $inSync = $advanced -and $row.model_verdict -eq 'in_sync' -and [int]$row.applied_sesno -eq [int]$row.file_latest_sesno
            if ($inSync) {
                $elapsed = [int]((Get-Date) - $markedAt).TotalSeconds
                $result = if ($elapsed -le $current.in_sync_budget) { 'PASS' } else { 'SLOW' }
                break
            }
            Start-Sleep -Seconds 5
        }
        $doneAt = Get-Date
        $elapsed = [int]($doneAt - $markedAt).TotalSeconds
        $endText = if ($row) { '{0}/{1}/{2}' -f $row.applied_sesno, $row.model_sesno, $row.file_latest_sesno } else { '—' }
        $manualText = if ($manualMoved) { '**动了**' } elseif ($null -eq $mark.manual_execute) { '未观察' } else { '没动' }
        if ($manualMoved -and $result -eq 'PASS') { $result = 'PASS-BUT-MANUAL' }
        $note = if ($row -and [int]$row.model_dead_roots -gt 0) { "已放弃根 $($row.model_dead_roots)（ISSUE-029 一类，非阻断，记账）" } else { '' }

        # L2-1：arm 过的 label 在收敛后自动对拍 HTTP 读面（变更根必须变、对照根逐字不变、
        # 无新失败行）。判据出处见 Http-IncrementAssert.ps1 头注释；超时不对拍——基线保留，
        # 收敛后可手动补跑 -Phase assert。
        $httpAssert = $null
        $capturePath = Join-Path $current.run_dir "marks\$Label-http-capture.json"
        if (Test-Path -LiteralPath $capturePath) {
            if ($result -eq 'TIMEOUT') {
                $httpAssert = 'NOT-RUN'
                Write-Host '未收敛，跳过 HTTP 读面对拍（基线保留，可手动补跑 Http-IncrementAssert.ps1 -Phase assert）' -ForegroundColor Yellow
            }
            else {
                try {
                    & (Join-Path $PSScriptRoot 'Http-IncrementAssert.ps1') -Phase assert -Label $Label -Dbnum $Dbnum
                    $assertExit = $LASTEXITCODE
                }
                catch {
                    Write-Host "HTTP 对拍抛错：$($_.Exception.Message)" -ForegroundColor Red
                    $assertExit = -1
                }
                $assertReportPath = Join-Path $current.run_dir "marks\$Label-http-assert.json"
                $httpAssert = if (Test-Path -LiteralPath $assertReportPath) {
                    (Get-Content -LiteralPath $assertReportPath -Raw | ConvertFrom-Json).overall
                }
                elseif ($assertExit -eq 0) { 'PASS' } else { "ERROR($assertExit)" }
            }
            $note = (@($note, "http-assert=$httpAssert") | Where-Object { $_ }) -join '；'
        }
        $ledgerLine = '| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} | {10} |' -f `
            $Label, $Dbnum, $markedAt.ToString('HH:mm:ss'), $startText,
            $(if ($result -ne 'TIMEOUT') { $doneAt.ToString('HH:mm:ss') } else { '—' }),
            $elapsed, $current.in_sync_budget, $result, $endText, $manualText, $note
        Add-Content -LiteralPath (Join-Path $current.run_dir 'ledger.md') -Value $ledgerLine -Encoding UTF8
        [ordered]@{
            label = $Label; dbnum = $Dbnum; marked_at = $mark.marked_at; done_at = $doneAt.ToString('o')
            elapsed_secs = $elapsed; budget_secs = $current.in_sync_budget; result = $result
            start = $start; end = $row; manual_moved = $manualMoved; note = $note
            http_assert = $httpAssert
            health = $health
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $current.run_dir "marks\$Label-result.json") -Encoding UTF8
        $color = switch ($result) { 'PASS' { 'Green' } 'SLOW' { 'Yellow' } default { 'Red' } }
        Write-Host ("{0}：{1} 用时 {2} s（预算 {3} s），终点 {4}，手动执行 {5} {6}" -f $result, $Label, $elapsed, $current.in_sync_budget, $endText, $manualText, $note) -ForegroundColor $color
        if ($result -eq 'TIMEOUT') { exit 2 }
        if ($httpAssert -eq 'FAIL') { exit 3 }
    }

    'report' {
        $current = Read-Current
        Get-Content -LiteralPath (Join-Path $current.run_dir 'ledger.md')
        Write-Host ''
        Write-Host "运行目录：$($current.run_dir)（marks/*.json 有每一拍的 /dbnums 与 /health）"
    }
}
