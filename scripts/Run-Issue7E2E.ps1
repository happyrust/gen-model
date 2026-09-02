[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$Datastore = 'rocksdb:.surreal/ams-7997-e3d-test-20260805',
    [string]$Output = "output/issue7-e2e/$(Get-Date -Format yyyyMMdd-HHmmss)"
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $Output
New-Item -ItemType Directory -Force $out | Out-Null
$startedSurreal = $null
$applied = $false

function Invoke-Driver([string]$macro, [string]$name) {
    & $script:l3 --check-driver $macro --project-dir $ProjectDir --output (Join-Path $out $name)
    if ($LASTEXITCODE) { throw "E3D driver failed: $macro" }
}

function Invoke-Increment([string]$name) {
    & cargo test --features http_api --test issue7_e2e_increment -j 1 `
        issue7_e2e_room_comes_back_after_e3d_save -- --ignored --exact --nocapture 2>&1 |
        Tee-Object -FilePath (Join-Path $out "$name.log")
    if ($LASTEXITCODE) { throw "Issue #7 increment test failed: $name" }
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
    $env:AIOS_ISSUE7_DB_FILE = Join-Path $ProjectDir 'ams000\ams7999_0001'
    $env:DB_OPTION_FILE = 'db_options/DbOption-issue7-e2e'
    # GEN_MODEL_DIRECT_INCREMENT 随 kv-mem 暂存窗口退役（ADR-056 P1）：稳态增量只有直写一条路，
    # 二进制不再读这个变量。

    & cargo build --bin l3_suite -j 1
    if ($LASTEXITCODE) { throw 'l3_suite build failed' }
    $target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
    $script:l3 = Join-Path $target 'debug\l3_suite.exe'

    Invoke-Driver 'scripts/e3d/issue7_cap_pos_apply.mac' 'apply-driver'
    $applied = $true
    Invoke-Increment 'apply-increment'
}
finally {
    if ($applied) {
        Invoke-Driver 'scripts/e3d/issue7_cap_pos_restore.mac' 'restore-driver'
        Invoke-Increment 'restore-increment'
    }
    if ($startedSurreal) {
        Stop-Process -Id $startedSurreal.Id -Force -ErrorAction SilentlyContinue
    }
    Pop-Location
}

Write-Host "Issue #7 E2E PASS: $out"
