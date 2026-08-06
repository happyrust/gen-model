[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$Datastore = 'rocksdb:.surreal/ams-7997-e3d-test-20260805',
    [string[]]$Cases = @('same-room', 'element-out', 'room-rename'),
    [string]$Output = "output/room-e3d-e2e/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $Output
New-Item -ItemType Directory -Force $out | Out-Null
$startedSurreal = $null

function Invoke-Driver([string]$macro, [string]$dir) {
    & $script:l3 --check-driver $macro --project-dir $ProjectDir --output (Join-Path $out $dir)
    if ($LASTEXITCODE) { throw "E3D driver failed: $macro" }
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
    [bool]$expectRoom,
    [bool]$prepareBaseline,
    [bool]$deleteBaseline
) {
    $env:AIOS_ROOM_CHANGE = $change
    $env:AIOS_ROOM_DBNUM = "$dbnum"
    $env:AIOS_ROOM_DB_FILE = Join-Path $ProjectDir "ams000\ams${dbnum}_0001"
    $env:AIOS_ROOM_EXPECT_ROOM = if ($expectRoom) { '1' } else { '0' }
    $env:AIOS_ROOM_PREPARE_BASELINE = if ($prepareBaseline) { '1' } else { '0' }
    $env:AIOS_ROOM_DELETE_BASELINE = if ($deleteBaseline) { '1' } else { '0' }
}

function Invoke-Case(
    [string]$name,
    [string]$change,
    [int]$dbnum,
    [string]$applyMacro,
    [string]$restoreMacro,
    [bool]$applyExpectsRoom,
    [bool]$deleteBaseline
) {
    $restoreRequired = $false
    try {
        Set-CaseEnv $change $dbnum $applyExpectsRoom $true $deleteBaseline
        # The macro may SAVEWORK before the driver reports a later cleanup error.
        $restoreRequired = $true
        Invoke-Driver $applyMacro "$name-apply-driver"
        Invoke-Increment "$name-apply-increment"
    }
    finally {
        if ($restoreRequired) {
            Invoke-Driver $restoreMacro "$name-restore-driver"
            Set-CaseEnv $change $dbnum $true $false $deleteBaseline
            Invoke-Increment "$name-restore-increment"
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

    foreach ($case in $Cases) {
        switch ($case) {
            'same-room' {
                Invoke-Case $case 'element' 7999 `
                    'scripts/e3d/issue7_cap_pos_apply.mac' `
                    'scripts/e3d/issue7_cap_pos_restore.mac' $true $true
            }
            'element-out' {
                Invoke-Case $case 'element' 7999 `
                    'scripts/e3d/room_cap_out_apply.mac' `
                    'scripts/e3d/room_cap_out_restore.mac' $false $false
            }
            'room-rename' {
                Invoke-Case $case 'room' 7997 `
                    'scripts/e3d/room_name_out_apply.mac' `
                    'scripts/e3d/room_name_out_restore.mac' $false $false
            }
            default { throw "Unknown room E2E case: $case" }
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
