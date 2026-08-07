[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$Datastore = 'rocksdb:.surreal/ams-7997-e3d-test-20260805',
    [string[]]$Cases = @('same-room', 'element-out', 'room-rename', 'box-size', 'cyli-size'),
    [string[]]$ModelTypes = @(),
    [switch]$SkipLegacyCases,
    [string]$Output = "output/room-e3d-e2e/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $Output
New-Item -ItemType Directory -Force $out | Out-Null
$startedSurreal = $null

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
    & cargo test --features http_api --test issue7_e2e_increment -j 1 `
        issue7_e2e_room_comes_back_after_e3d_save -- --ignored --exact --nocapture 2>&1 |
        Tee-Object -FilePath (Join-Path $out "$dir.log")
    if ($LASTEXITCODE) { throw "Room increment test failed: $dir" }
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
    [bool]$checkTopology = $true
) {
    $env:AIOS_ROOM_CHANGE = $change
    $env:AIOS_ROOM_ELEMENT = $element
    $env:AIOS_ROOM_EXPECT_NOUN = $noun
    $env:AIOS_ROOM_DBNUM = "$dbnum"
    $env:AIOS_ROOM_DB_FILE = Join-Path $ProjectDir "ams000\ams${dbnum}_0001"
    $env:AIOS_ROOM_EXPECT_ROOM = if ($expectRoom) { '1' } else { '0' }
    $env:AIOS_ROOM_PREPARE_BASELINE = if ($prepareBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_DELETE_BASELINE = if ($deleteBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_DYNAMIC_BASELINE = if ($dynamicBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_CHECK_TOPOLOGY = if ($checkTopology) { '1' } else { '0' }
    $env:AIOS_ROOM_KEYWORD = if ($dynamicBaseline) { '-RM' } else { '-RM05-R512' }
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
    [bool]$checkTopology = $true
) {
    $restoreRequired = $false
    try {
        Set-CaseEnv $change $dbnum $element $noun $applyExpectsRoom $true $deleteBaseline $dynamicBaseline $checkTopology
        # The macro may SAVEWORK before the driver reports a later cleanup error.
        $restoreRequired = $true
        Invoke-Driver $applyMacro "$name-apply-driver"
        Invoke-Increment "$name-apply-increment"
    }
    finally {
        if ($restoreRequired) {
            try {
                Invoke-Driver $restoreMacro "$name-restore-driver"
            }
            finally {
                $restoreExpectsRoom = if ($dynamicBaseline) { $applyExpectsRoom } else { $true }
                Set-CaseEnv $change $dbnum $element $noun $restoreExpectsRoom $false $deleteBaseline $dynamicBaseline $checkTopology
                Invoke-Increment "$name-restore-increment"
            }
        }
    }
}

Push-Location $repo
try {
    if (-not (Test-NetConnection 127.0.0.1 -Port 8009 -InformationLevel Quiet -WarningAction SilentlyContinue)) {
        $startedSurreal = Start-Process (Join-Path $repo 'bin/surreal.exe') -WindowStyle Hidden -PassThru `
            -WorkingDirectory $repo -ArgumentList @('start', '--user', 'root', '--pass', 'root',
                '--bind', '127.0.0.1:8009', $Datastore) `
            -RedirectStandardOutput (Join-Path $out 'surreal.stdout.log') `
            -RedirectStandardError (Join-Path $out 'surreal.stderr.log')
        foreach ($null in 1..90) {
            if (Test-NetConnection 127.0.0.1 -Port 8009 -InformationLevel Quiet -WarningAction SilentlyContinue) { break }
            Start-Sleep 1
        }
        if (-not (Test-NetConnection 127.0.0.1 -Port 8009 -InformationLevel Quiet -WarningAction SilentlyContinue)) {
            throw 'SurrealDB did not open port 8009'
        }
    }

    $env:CARGO_BUILD_JOBS = '1'
    $env:RUSTFLAGS = '-Z threads=1'
    $env:RUST_MIN_STACK = '134217728'
    $env:AIOS_LIVE_WS = 'ws://127.0.0.1:8009'
    $env:AIOS_LIVE_NS = '1516'
    $env:AIOS_LIVE_DB = 'AvevaMarineSample'
    $env:DB_OPTION_FILE = 'db_options/DbOption-issue7-e2e'
    $env:GEN_MODEL_DIRECT_INCREMENT = '1'

    & cargo build --bin l3_suite -j 1
    if ($LASTEXITCODE) { throw 'l3_suite build failed' }
    $target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
    $script:l3 = Join-Path $target 'debug\l3_suite.exe'

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
            'room-rename' {
                Invoke-Case $case 'room' 7997 '24383_66460' 'CAP' `
                    'scripts/e3d/room_name_out_apply.mac' `
                    'scripts/e3d/room_name_out_restore.mac' $false $false
            }
            'box-size' {
                Invoke-Driver 'scripts/e3d/room_box_size_probe.mac' `
                    'box-size-preflight-driver' 'Xlength 100mm'
                Invoke-Case $case 'element' 7997 '24381_101446' 'BOX' `
                    'scripts/e3d/room_box_size_apply.mac' `
                    'scripts/e3d/room_box_size_restore.mac' $true $true
            }
            'cyli-size' {
                Invoke-Driver 'scripts/e3d/room_cyli_size_probe.mac' `
                    'cyli-size-preflight-driver' 'Diameter 50mm'
                Invoke-Case $case 'element' 7997 '24381_101426' 'CYLI' `
                    'scripts/e3d/room_cyli_size_apply.mac' `
                    'scripts/e3d/room_cyli_size_restore.mac' $true $true
            }
            default { throw "Unknown room E2E case: $case" }
        }
    } }

    if ($ModelTypes) {
        $manifest = Get-Content (Join-Path $repo 'scripts/e3d/ams_model_type_cases.json') -Raw |
            ConvertFrom-Json
        $selected = @(if ($ModelTypes -contains 'all') {
            $manifest | Where-Object mode -eq 'relative_position'
        } else {
            $manifest | Where-Object { $_.noun -in $ModelTypes }
        })
        if (($ModelTypes -notcontains 'all') -and $selected.Count -ne $ModelTypes.Count) {
            throw "Unknown or duplicate model type. Requested: $($ModelTypes -join ', ')"
        }
        $generated = Join-Path $out 'generated-macros'
        New-Item -ItemType Directory -Force $generated | Out-Null
        foreach ($model in $selected) {
            if ($model.mode -ne 'relative_position') { continue }
            $name = $model.id
            $refno = $model.refno -replace '_', '/'
            $selectLine = if ($model.selector) { "CE $($model.selector)" } else { "=$refno" }
            $applyCommand = if ($model.apply_command) { $model.apply_command } else { 'BY U 10' }
            $restoreCommand = if ($model.restore_command) { $model.restore_command } else { 'BY D 10' }
            $macroLog = ((Resolve-Path $generated).Path -replace '\\', '/')
            $applyMacro = Join-Path $generated "$name-apply.mac"
            $restoreMacro = Join-Path $generated "$name-restore.mac"
            @"
ALPHA LOG "$macroLog/$name-apply.log" OVER
$selectLine
Q CE
Q POS
$applyCommand
Q POS
SAVEWORK 'CODEX $name relative position apply'
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
            Invoke-Case $name 'element' $model.dbnum $model.refno $model.noun `
                $applyMacro $restoreMacro $model.expect_room $model.expect_room $true $false
        }
    }
}
finally {
    if ($startedSurreal) {
        Stop-Process -Id $startedSurreal.Id -Force -ErrorAction SilentlyContinue
    }
    Pop-Location
}

Write-Host "Room E3D E2E PASS: $out"
