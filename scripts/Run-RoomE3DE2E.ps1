[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$Datastore = 'rocksdb:.surreal/ams-7997-e3d-test-20260805',
    [string[]]$Cases = @('same-room', 'element-out', 'cross-db-room', 'room-rename', 'box-size', 'cyli-size'),
    [string[]]$ModelTypes = @(),
    [switch]$SkipLegacyCases,
    [switch]$DirectIncrement,
    [int]$SurrealPort = 8009,
    [switch]$ReuseExistingDatabase,
    [string]$TestExe = '',
    [string]$L3Exe = '',
    [string]$Output = "output/room-e3d-e2e/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $Output
New-Item -ItemType Directory -Force $out | Out-Null
$startedSurreal = $null
$script:databaseSource = $Datastore
# 逐案结果（report.md 的数据源）。失败隔离纪律：断言失败只 FAIL 本案例，restore 照跑、
# 整轮继续；restore 链路失败 = 库基线可疑 = FATAL，立即终止整轮（房间计划 RI-15）。
$script:results = New-Object System.Collections.Generic.List[object]
$script:fatal = ''

function Invoke-Driver([string]$macro, [string]$dir, [string]$expected = '') {
    & $script:l3 --check-driver $macro --project-dir $ProjectDir --output (Join-Path $out $dir)
    if ($LASTEXITCODE) { throw "E3D driver failed: $macro" }
    if ($expected) {
        $log = Get-Content (Join-Path $out "$dir/check-driver.log") -Raw
        if ($log -notmatch [regex]::Escape($expected)) {
            throw "E3D driver output did not contain '$expected': $macro"
        }
    }
}

function Invoke-Increment([string]$dir) {
    # Windows PowerShell 5.1 wraps every native stderr line as a
    # RemoteException. With this script's global ErrorActionPreference=Stop,
    # harmless Rust eprintln!/dbg! diagnostics used to abort the pipeline before
    # the test process returned an exit code. Keep streaming both handles into
    # the evidence log, but decide success exclusively from LASTEXITCODE.
    $previousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($TestExe) {
            & $TestExe issue7_e2e_room_comes_back_after_e3d_save --ignored --exact --nocapture 2>&1 |
                Tee-Object -FilePath (Join-Path $out "$dir.log")
        } else {
            & cargo test --features http_api --test issue7_e2e_increment -j 1 `
                issue7_e2e_room_comes_back_after_e3d_save -- --ignored --exact --nocapture 2>&1 |
                Tee-Object -FilePath (Join-Path $out "$dir.log")
        }
        $testExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($testExitCode) { throw "Room increment test failed: $dir (exit $testExitCode)" }
}

function New-CaseRow([string]$name) {
    [ordered]@{
        name         = $name
        status       = 'PASS'
        failed_phase = ''
        error        = ''
        assertion    = ''
        phases       = New-Object System.Collections.Generic.List[string]
    }
}

function Invoke-Phase($row, [string]$phase, [scriptblock]$body) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $body
        $row.phases.Add(('{0} {1:n0}s' -f $phase, $sw.Elapsed.TotalSeconds))
    } catch {
        $row.phases.Add(('{0} {1:n0}s FAIL' -f $phase, $sw.Elapsed.TotalSeconds))
        if (-not $row.failed_phase) { $row.failed_phase = $phase }
        throw
    }
}

# 从增量测试日志里捞第一条失败断言（report 的「首个失败判据」列）。
function Get-FirstAssertion([string]$dir) {
    $log = Join-Path $out "$dir.log"
    if (-not (Test-Path -LiteralPath $log)) { return '' }
    $lines = @(Get-Content -LiteralPath $log)
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match 'panicked at') {
            $end = [Math]::Min($i + 2, $lines.Count - 1)
            return (($lines[$i..$end] -join ' ').Trim())
        }
    }
    return ''
}

function Format-Cell([string]$text) {
    if (-not $text) { return '' }
    $t = $text -replace '\r?\n', ' ' -replace '\|', '/'
    if ($t.Length -gt 220) { $t = $t.Substring(0, 220) + [char]0x2026 }
    return $t
}

function Write-Report {
    try {
        $lines = New-Object System.Collections.Generic.List[string]
        $lines.Add('# Room E3D E2E 轮次报告')
        $lines.Add('')
        $lines.Add('| 键 | 值 |')
        $lines.Add('|---|---|')
        $lines.Add("| 时间 | $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') |")
        $lines.Add("| 项目 | $ProjectDir |")
        $lines.Add("| database | $script:databaseSource |")
        $lines.Add("| legacy 用例 | $(if ($SkipLegacyCases) { '(跳过)' } else { $Cases -join ', ' }) |")
        $lines.Add("| 模型类型 | $(if ($ModelTypes) { $ModelTypes -join ', ' } else { '(无)' }) |")
        $lines.Add("| 增量路径 | $(if ($DirectIncrement) { '应急直写 + 手工 room drain' } else { 'ADR-017 staged + 提交后 scoped room drain' }) |")
        $lines.Add('| 幂等步骤 | restore 收敛后每案例一次（AIOS_ROOM_IDEMPOTENT=1，T-OR-3） |')
        if ($script:fatal) {
            $lines.Add("| **FATAL** | 恢复链路失败终止整轮（RI-15）：$(Format-Cell $script:fatal) |")
        }
        $lines.Add('')
        $pass = @($script:results | Where-Object status -eq 'PASS').Count
        $lines.Add("**结果：$pass / $($script:results.Count) PASS**")
        $lines.Add('')
        $lines.Add('| 用例 | 结果 | 失败阶段 | 首个失败断言/错误 | 阶段耗时 | 日志 |')
        $lines.Add('|---|---|---|---|---|---|')
        foreach ($r in $script:results) {
            $err = Format-Cell $(if ($r.assertion) { $r.assertion } else { $r.error })
            $lines.Add("| $($r.name) | $($r.status) | $($r.failed_phase) | $err | $($r.phases -join ' / ') | ``$($r.name)-*`` |")
        }
        $lines.Add('')
        $lines.Add("证据目录：``$out``")
        Set-Content -LiteralPath (Join-Path $out 'report.md') -Value $lines -Encoding utf8
        Write-Host "report: $(Join-Path $out 'report.md')"
    } catch {
        Write-Warning "写 report.md 失败: $_"
    }
}

function Set-CaseEnv(
    [string]$change,
    [int]$dbnum,
    [string]$element,
    [string]$noun,
    [bool]$expectRoom,
    [bool]$prepareBaseline,
    [bool]$deleteBaseline,
    [bool]$dynamicBaseline = $false,
    [bool]$checkTopology = $true,
    [string]$expectedEdges = '',
    [bool]$expectGeometry = $true,
    [string]$nounRefno = '',
    [bool]$checkMembership = $true
) {
    # 本套件验证的就是增量房间触发链；仓库默认配置刻意关闭该功能，若不在
    # 测试进程显式打开，prepare 阶段遗留的 pending 行会让部分案例假通过，
    # 而纯 Transform 案例则报“未排队”。
    $env:AIOS_ROOM_INCREMENTAL = '1'
    # 专用房间用例默认走生产的 ADR-017 staged 路径，并要求数据批次
    # 返回前已 scoped 消化本窗口 room work。只有显式 -DirectIncrement，或纯
    # 模型类型覆盖（故意不验房间归属），才使用应急直写。
    $useDirectIncrement = $DirectIncrement -or (-not $checkMembership)
    $env:GEN_MODEL_DIRECT_INCREMENT = if ($useDirectIncrement) { '1' } else { '0' }
    $env:AIOS_ROOM_EXPECT_POSTCOMMIT_DRAIN = if ($useDirectIncrement) { '0' } else { '1' }
    $env:AIOS_ROOM_CHANGE = $change
    $env:AIOS_ROOM_ELEMENT = $element
    $env:AIOS_ROOM_EXPECT_NOUN = $noun
    $env:AIOS_ROOM_EXPECT_NOUN_REFNO = if ($nounRefno) { $nounRefno } else { $element }
    $env:AIOS_ROOM_DBNUM = "$dbnum"
    $env:AIOS_ROOM_DB_FILE = Join-Path $ProjectDir "ams000\ams${dbnum}_0001"
    $env:AIOS_ROOM_EXPECT_ROOM = if ($expectRoom) { '1' } else { '0' }
    $env:AIOS_ROOM_PREPARE_BASELINE = if ($prepareBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_DELETE_BASELINE = if ($deleteBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_DYNAMIC_BASELINE = if ($dynamicBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_CHECK_TOPOLOGY = if ($checkTopology) { '1' } else { '0' }
    $env:AIOS_ROOM_CHECK_MEMBERSHIP = if ($checkMembership) { '1' } else { '0' }
    $env:AIOS_ROOM_KEYWORD = if ($dynamicBaseline) { '-RM' } else { '-RM05-R512' }
    if ($expectedEdges) { $env:AIOS_ROOM_EXPECT_EDGES = $expectedEdges }
    else { Remove-Item Env:AIOS_ROOM_EXPECT_EDGES -ErrorAction SilentlyContinue }
    $env:AIOS_ROOM_EXPECT_GEOMETRY = if ($expectGeometry) { '1' } else { '0' }
}

function Invoke-Case(
    [string]$name,
    [string]$change,
    [int]$dbnum,
    [string]$element,
    [string]$noun,
    [string]$applyMacro,
    [string]$restoreMacro,
    [bool]$applyExpectsRoom,
    [bool]$deleteBaseline,
    [bool]$dynamicBaseline = $false,
    [bool]$checkTopology = $true,
    [string]$applyExpectedEdges = '',
    [string]$restoreExpectedEdges = '',
    [bool]$expectGeometry = $true,
    [string]$nounRefno = '',
    [string]$applySavedMarker = '',
    [bool]$checkMembership = $true
) {
    $row = New-CaseRow $name
    $restoreRequired = $false
    $envReady = $false
    $env:AIOS_ROOM_BASELINE_FILE = Join-Path $out "$name-baseline.json"
    Remove-Item -LiteralPath $env:AIOS_ROOM_BASELINE_FILE -ErrorAction SilentlyContinue
    if ($applySavedMarker) { Remove-Item -LiteralPath $applySavedMarker -ErrorAction SilentlyContinue }
    try {
        try {
            # 基线必须在 E3D apply 宏之前建立。宏 SAVEWORK 后文件已是新状态、而
            # Surreal 水位仍是旧状态；此时再强制生成会把两个 session 混成一份模型。
            Set-CaseEnv $change $dbnum $element $noun $applyExpectsRoom $true $false $dynamicBaseline $checkTopology $applyExpectedEdges $expectGeometry $nounRefno $checkMembership
            $envReady = $true
            $env:AIOS_ROOM_PREPARE_ONLY = '1'
            Remove-Item Env:AIOS_ROOM_RESTORE_PHASE -ErrorAction SilentlyContinue
            try {
                Invoke-Phase $row 'baseline-increment' { Invoke-Increment "$name-baseline-increment" }
            }
            finally {
                Remove-Item Env:AIOS_ROOM_PREPARE_ONLY -ErrorAction SilentlyContinue
            }
            # 旧宏没有 SAVEWORK 哨兵，沿用保守恢复；生成宏仅在确认 SAVEWORK 后恢复。
            $restoreRequired = -not $applySavedMarker
            try {
                Invoke-Phase $row 'apply-driver' { Invoke-Driver $applyMacro "$name-apply-driver" }
                $restoreRequired = $true
            } finally {
                if ($applySavedMarker -and (Test-Path -LiteralPath $applySavedMarker)) {
                    $restoreRequired = $true
                }
            }
            Set-CaseEnv $change $dbnum $element $noun $applyExpectsRoom $false $deleteBaseline $dynamicBaseline $checkTopology $applyExpectedEdges $expectGeometry $nounRefno $checkMembership
            Remove-Item Env:AIOS_ROOM_RESTORE_PHASE -ErrorAction SilentlyContinue
            Invoke-Phase $row 'apply-increment' { Invoke-Increment "$name-apply-increment" }
        } catch {
            if (-not $envReady) {
                # 环境都没设起来：配置级故障，不属于可隔离的场景失败。
                $row.status = 'FATAL'
                $row.error = "$_"
                $script:fatal = "${name}: setup: $_"
                throw
            }
            # T-OR-1：apply 侧失败只 FAIL 本案例；restore 照跑，整轮继续。
            $row.status = 'FAIL'
            $row.error = "$_"
            $row.assertion = Get-FirstAssertion "$name-$($row.failed_phase)"
            Write-Warning "case ${name} FAIL（已隔离，restore 照跑）: $_"
        }
        if ($restoreRequired) {
            try {
                $restoreDriverError = $null
                try {
                    Invoke-Phase $row 'restore-driver' { Invoke-Driver $restoreMacro "$name-restore-driver" }
                } catch {
                    # restore 宏可能在 driver 报清理错误前已 SAVEWORK：恢复增量照跑，
                    # 由它判定库是否真的回到基线。
                    $restoreDriverError = $_
                }
                $restoreExpectsRoom = if ($dynamicBaseline) { $applyExpectsRoom } else { $true }
                Set-CaseEnv $change $dbnum $element $noun $restoreExpectsRoom $false $deleteBaseline $dynamicBaseline $checkTopology $restoreExpectedEdges $expectGeometry $nounRefno $checkMembership
                $env:AIOS_ROOM_RESTORE_PHASE = '1'
                Invoke-Phase $row 'restore-increment' { Invoke-Increment "$name-restore-increment" }
                # T-OR-3：restore 收敛后第二遍必须是无操作（零批次、水位/边/AABB 不动）。
                $env:AIOS_ROOM_IDEMPOTENT = '1'
                try {
                    Invoke-Phase $row 'idempotent-increment' { Invoke-Increment "$name-idempotent-increment" }
                }
                finally {
                    Remove-Item Env:AIOS_ROOM_IDEMPOTENT -ErrorAction SilentlyContinue
                }
                if ($restoreDriverError -and $row.status -eq 'PASS') {
                    # 数据已验证回基线，但通道收尾不干净：记 FAIL，不升级 FATAL。
                    $row.status = 'FAIL'
                    $row.error = "restore driver: $restoreDriverError"
                }
            } catch {
                # RI-15：恢复增量没收敛（或幂等轮发现库还在动）→ 基线可疑，终止整轮。
                $row.status = 'FATAL'
                if (-not $row.error) { $row.error = "$_" }
                if (-not $row.assertion) { $row.assertion = Get-FirstAssertion "$name-$($row.failed_phase)" }
                $script:fatal = "${name}: $($row.failed_phase): $_"
                throw
            }
        }
    }
    finally {
        $script:results.Add([pscustomobject]$row)
    }
}

# 尺寸类用例的前置探针失败 → 记 FAIL 并跳过该用例，不中止整轮。
function Invoke-Preflight([string]$name, [string]$macro, [string]$dir, [string]$expected) {
    try {
        Invoke-Driver $macro $dir $expected
        return $true
    } catch {
        $row = New-CaseRow $name
        $row.status = 'FAIL'
        $row.failed_phase = 'preflight-driver'
        $row.error = "$_"
        $script:results.Add([pscustomobject]$row)
        Write-Warning "case ${name} preflight FAIL（跳过该用例）: $_"
        return $false
    }
}

Push-Location $repo
try {
    $portOpen = Test-NetConnection 127.0.0.1 -Port $SurrealPort -InformationLevel Quiet -WarningAction SilentlyContinue
    if ($portOpen) {
        if (-not $ReuseExistingDatabase) {
            throw "SurrealDB port $SurrealPort is already occupied. Pass -ReuseExistingDatabase to use the current 1516 database explicitly."
        }
        $script:databaseSource = "existing ws://127.0.0.1:$SurrealPort (1516/AvevaMarineSample)"
        $consumers = @(Get-Process -Name 'aios-database' -ErrorAction SilentlyContinue)
        if ($consumers.Count) {
            $consumerPids = ($consumers | ForEach-Object Id) -join ', '
            throw "Concurrent aios-database consumer detected (PID $consumerPids). Stop it before reusing the current 1516 database; otherwise it can steal the E3D session before this test."
        }
    } else {
        $startedSurreal = Start-Process (Join-Path $repo 'bin/surreal.exe') -WindowStyle Hidden -PassThru `
            -WorkingDirectory $repo -ArgumentList @('start', '--user', 'root', '--pass', 'root',
                '--bind', "127.0.0.1:$SurrealPort", $Datastore) `
            -RedirectStandardOutput (Join-Path $out 'surreal.stdout.log') `
            -RedirectStandardError (Join-Path $out 'surreal.stderr.log')
        foreach ($null in 1..90) {
            if (Test-NetConnection 127.0.0.1 -Port $SurrealPort -InformationLevel Quiet -WarningAction SilentlyContinue) { break }
            Start-Sleep 1
        }
        if (-not (Test-NetConnection 127.0.0.1 -Port $SurrealPort -InformationLevel Quiet -WarningAction SilentlyContinue)) {
            throw "SurrealDB did not open port $SurrealPort"
        }
    }

    $env:CARGO_BUILD_JOBS = '1'
    $env:RUSTFLAGS = '-Z threads=1'
    $env:RUST_MIN_STACK = '134217728'
    $env:AIOS_LIVE_WS = "ws://127.0.0.1:$SurrealPort"
    $env:AIOS_LIVE_NS = '1516'
    $env:AIOS_LIVE_DB = 'AvevaMarineSample'
    $env:DB_OPTION_FILE = 'db_options/DbOption-issue7-e2e'

    if ($L3Exe) {
        $script:l3 = (Resolve-Path $L3Exe).Path
    } else {
        & cargo build --bin l3_suite -j 1
        if ($LASTEXITCODE) { throw 'l3_suite build failed' }
        $target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
        $script:l3 = Join-Path $target 'debug\l3_suite.exe'
    }

    if (-not $SkipLegacyCases) { foreach ($case in $Cases) {
        switch ($case) {
            'same-room' {
                Invoke-Case $case 'element' 7999 '24383_66460' 'CAP' `
                    'scripts/e3d/issue7_cap_pos_apply.mac' `
                    'scripts/e3d/issue7_cap_pos_restore.mac' $true $true
            }
            'element-out' {
                Invoke-Case $case 'element' 7999 '24383_66460' 'CAP' `
                    'scripts/e3d/room_cap_out_apply.mac' `
                    'scripts/e3d/room_cap_out_restore.mac' $false $false
            }
            'cross-db-room' {
                # 跨库房间迁移：CAP（db7999）从 R512（房间 FRMW 与面板在 db7997）搬进
                # /6KA-RM01-K101（房间树在 db1112，SITE /6KA-ARCH）。K170 公共区域体
                # （/6KA-RM01-K170，面板 17496_230648）完全包含 K101，双归属是该区域
                # 常态（定标证据：K101 现有成员 2/3 同时挂 K170 边），期望边按两条冻结。
                # 动态基线 + '-RM' 关键字：房间图必须同时纳入 1RX（7997）与 6KA（1112）。
                Invoke-Case $case 'element' 7999 '24383_66460' 'CAP' `
                    'scripts/e3d/room_cap_cross_db_apply.mac' `
                    'scripts/e3d/room_cap_cross_db_restore.mac' $true $true $true $false `
                    '[{"panel":"17496_230552","part":"24383_66460","room_num":"K101"},{"panel":"17496_230648","part":"24383_66460","room_num":"K170"}]' `
                    '[{"panel":"24381_35844","part":"24383_66460","room_num":"R512"}]'
            }
            'room-rename' {
                Invoke-Case $case 'room' 7997 '24383_66460' 'CAP' `
                    'scripts/e3d/room_name_out_apply.mac' `
                    'scripts/e3d/room_name_out_restore.mac' $false $false
            }
            'box-size' {
                if (Invoke-Preflight $case 'scripts/e3d/room_box_size_probe.mac' `
                        'box-size-preflight-driver' 'Xlength 100mm') {
                    Invoke-Case $case 'element' 7997 '24381_101446' 'BOX' `
                        'scripts/e3d/room_box_size_apply.mac' `
                        'scripts/e3d/room_box_size_restore.mac' $true $true
                }
            }
            'cyli-size' {
                if (Invoke-Preflight $case 'scripts/e3d/room_cyli_size_probe.mac' `
                        'cyli-size-preflight-driver' 'Diameter 50mm') {
                    Invoke-Case $case 'element' 7997 '24381_101426' 'CYLI' `
                        'scripts/e3d/room_cyli_size_apply.mac' `
                        'scripts/e3d/room_cyli_size_restore.mac' $true $true
                }
            }
            default { throw "Unknown room E2E case: $case" }
        }
    } }

    if ($ModelTypes) {
        $manifest = Get-Content (Join-Path $repo 'scripts/e3d/ams_model_type_cases.json') -Raw |
            ConvertFrom-Json
        $selected = @(if ($ModelTypes -contains 'all') {
            $manifest
        } else {
            $manifest | Where-Object { $_.noun -in $ModelTypes }
        })
        if (($ModelTypes -notcontains 'all') -and $selected.Count -ne $ModelTypes.Count) {
            throw "Unknown or duplicate model type. Requested: $($ModelTypes -join ', ')"
        }
        $generated = Join-Path $out 'generated-macros'
        New-Item -ItemType Directory -Force $generated | Out-Null
        foreach ($model in $selected) {
            if ($model.mode -eq 'existing') {
                switch ($model.noun) {
                    'CAP' {
                        Invoke-Case $model.id 'element' 7999 '24383_66460' 'CAP' `
                            'scripts/e3d/issue7_cap_pos_apply.mac' 'scripts/e3d/issue7_cap_pos_restore.mac' $true $true
                    }
                    'BOX' {
                        if (Invoke-Preflight $model.id 'scripts/e3d/room_box_size_probe.mac' "$($model.id)-preflight-driver" 'Xlength 100mm') {
                            Invoke-Case $model.id 'element' 7997 '24381_101446' 'BOX' `
                                'scripts/e3d/room_box_size_apply.mac' 'scripts/e3d/room_box_size_restore.mac' $true $true
                        }
                    }
                    'CYLI' {
                        if (Invoke-Preflight $model.id 'scripts/e3d/room_cyli_size_probe.mac' "$($model.id)-preflight-driver" 'Diameter 50mm') {
                            Invoke-Case $model.id 'element' 7997 '24381_101426' 'CYLI' `
                                'scripts/e3d/room_cyli_size_apply.mac' 'scripts/e3d/room_cyli_size_restore.mac' $true $true
                        }
                    }
                    default { throw "Unsupported existing model type: $($model.noun)" }
                }
                continue
            }
            if ($model.mode -ne 'relative_position') { throw "Unknown model type mode: $($model.mode)" }
            $name = $model.id
            $refno = $model.refno -replace '_', '/'
            $elementRefno = if ($model.element_refno) { $model.element_refno } else { $model.refno }
            $selectLine = if ($model.select_command) { $model.select_command } elseif ($model.selector) { "CE $($model.selector)" } else { "=$($elementRefno -replace '_', '/')" }
            $applyCommand = if ($model.apply_command) { $model.apply_command } else { 'BY U 10' }
            $restoreCommand = if ($model.restore_command) { $model.restore_command } else { 'BY D 10' }
            $macroLog = ((Resolve-Path $generated).Path -replace '\\', '/')
            $applyMacro = Join-Path $generated "$name-apply.mac"
            $restoreMacro = Join-Path $generated "$name-restore.mac"
            $applySavedMarker = "$applyMacro.saved"
            $applySavedMarkerE3d = ($applySavedMarker -replace '\\', '/')
            @"
ALPHA LOG "$macroLog/$name-apply.log" OVER
$selectLine
Q CE
Q POS
$applyCommand
Q POS
SAVEWORK 'CODEX $name relative position apply'
ALPHA LOG END
ALPHA LOG "$applySavedMarkerE3d" OVER
Q CE
ALPHA LOG END
"@ | Set-Content $applyMacro -Encoding ascii
            @"
ALPHA LOG "$macroLog/$name-restore.log" OVER
$selectLine
Q CE
Q POS
$restoreCommand
Q POS
SAVEWORK 'CODEX $name relative position restore'
ALPHA LOG END
"@ | Set-Content $restoreMacro -Encoding ascii
            $edgeJson = {
                param($items)
                if (-not $items) { return '' }
                @(foreach ($edge in $items) {
                    [ordered]@{ panel = $edge.panel; part = $elementRefno; room_num = $edge.room }
                }) | ConvertTo-Json -Compress -AsArray
            }
            $applyExpectedEdges = & $edgeJson $model.apply_expected_edges
            $restoreExpectedEdges = & $edgeJson $model.restore_expected_edges
            $expectGeometry = if ($null -eq $model.expect_geometry) { $true } else { [bool]$model.expect_geometry }
            $change = if ($model.noun -eq 'PANE') { 'room' } else { 'element' }
            $checkTopology = $model.noun -eq 'PANE'
            Invoke-Case $name $change $model.dbnum $elementRefno $model.noun `
                $applyMacro $restoreMacro $model.expect_room $model.expect_room $true $checkTopology `
                $applyExpectedEdges $restoreExpectedEdges $expectGeometry $model.refno $applySavedMarker $false
        }
    }
}
finally {
    Write-Report
    if ($startedSurreal) {
        Stop-Process -Id $startedSurreal.Id -Force -ErrorAction SilentlyContinue
    }
    Pop-Location
}

$failed = @($script:results | Where-Object status -ne 'PASS')
if ($failed.Count) {
    throw "Room E3D E2E FAIL（$($failed.Count)/$($script:results.Count)），详见 $(Join-Path $out 'report.md')"
}
Write-Host "Room E3D E2E PASS: $out"
