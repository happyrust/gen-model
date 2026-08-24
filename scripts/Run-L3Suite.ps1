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
    # 一条命令建两个 bin：分两次建的话第二条用的是默认特征集，整个 lib 会因为特征
    # 不统一而重编一遍（l3_suite 一行 lib 代码都没用到，纯白等）。
    cargo build --bin aios-database --bin l3_suite --no-default-features --features "ws,gen_model,manifold,project_hd,http_api"
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    & (Join-Path $target "debug/l3_suite.exe") @RunnerArgs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
