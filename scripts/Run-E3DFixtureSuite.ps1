[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$TargetDbFile,
    [Parameter(Mandatory)][int]$TargetDbnum,
    [Parameter(Mandatory)][string]$ProjectDir,
    [Parameter(Mandatory)][string]$AiosProject,
    [Parameter(Mandatory)][string]$AiosNamespace,
    [Parameter(Mandatory)][string]$E3dProject,
    [string]$E3dLogin = "SYSTEM/XXXXXX",
    [string]$E3dMdb = "/ALL",
    [string]$ProjectEvar,
    [string]$Output,
    [switch]$Ui,
    [switch]$KeepSites,
    [switch]$KeepStack
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo "target" }
Push-Location $repo
try {
    cargo build --bin aios-database --bin l3_suite --bin sync_sys_only `
        --bin initialize_ams_dbnums --bin manual_scan_probe `
--no-default-features --features "ws,gen_model,manifold,project_hd,http_api"
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    # The baseline bootstrap walks a deep ownership graph on the process main
    # thread. Rust's RUST_MIN_STACK only covers spawned threads, so reserve the
    # same 128 MiB on the PE header before starting the unattended service.
    $editbin = Get-Command editbin.exe -ErrorAction SilentlyContinue
    if (-not $editbin) {
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $vswhere) {
            $editbinPath = & $vswhere -latest -products '*' -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\editbin.exe' | Select-Object -First 1
            if ($editbinPath) { $editbin = Get-Item -LiteralPath $editbinPath }
        }
    }
    if (-not $editbin) { throw 'editbin.exe is required to set the aios-database main-thread stack' }
    $editbinExe = if ($editbin.Source) { $editbin.Source } else { $editbin.FullName }
    & $editbinExe /STACK:134217728 (Join-Path $target 'debug/aios-database.exe')
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    $runnerArgs = @(
        "--fixture-manifest", "scripts/e3d/increment_fixture/fixture-manifest.json",
        "--target-db-file", $TargetDbFile,
        "--target-dbnum", "$TargetDbnum",
        "--project-dir", $ProjectDir,
        "--aios-project", $AiosProject,
        "--aios-namespace", $AiosNamespace,
        "--e3d-project", $E3dProject,
        "--e3d-login", $E3dLogin,
        "--e3d-mdb", $E3dMdb
    )
    if ($ProjectEvar) { $runnerArgs += @("--project-evar", $ProjectEvar) }
    if ($Output) { $runnerArgs += @("--output", $Output) }
    if ($Ui) { $runnerArgs += "--fixture-ui" }
    if ($KeepSites) { $runnerArgs += "--fixture-keep-sites" }
    if ($KeepStack) { $runnerArgs += "--keep-stack" }
    & (Join-Path $target "debug/l3_suite.exe") @runnerArgs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
