[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$RunnerArgs
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo "target" }

Push-Location $repo
try {
    cargo build --bin aios-database --no-default-features --features "ws,gen_model,manifold,occ,project_hd,http_api"
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    cargo build --bin l3_suite
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    & (Join-Path $target "debug/l3_suite.exe") @RunnerArgs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
