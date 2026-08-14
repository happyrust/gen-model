param(
    [Parameter(Mandatory = $true)]
    [string]$DeployDir,
    [string]$TestWorkspace = "D:\work\plant-code\old\test-worklspace"
)

$ErrorActionPreference = "Stop"
$deploy = (Resolve-Path -LiteralPath $DeployDir).Path
$workspace = (Resolve-Path -LiteralPath $TestWorkspace).Path
$bin = Join-Path $workspace "bin"
$baseline = Join-Path $deploy "aios-database.baseline.exe"
$config = Join-Path $deploy "DbOption.original.toml"

if (-not (Test-Path -LiteralPath $baseline) -or -not (Test-Path -LiteralPath $config)) {
    throw "rollback inputs are incomplete under $deploy"
}

Get-CimInstance Win32_Process |
    Where-Object { $_.ExecutablePath -eq (Join-Path $bin "aios-database.exe") } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }

Copy-Item -LiteralPath $baseline -Destination (Join-Path $bin "aios-database.exe") -Force
Copy-Item -LiteralPath $config -Destination (Join-Path $bin "DbOption.toml") -Force

$sha = [System.Security.Cryptography.SHA256]::Create()
$stream = [IO.File]::OpenRead((Join-Path $bin "aios-database.exe"))
try {
    $hash = [Convert]::ToHexString($sha.ComputeHash($stream)).ToLowerInvariant()
} finally {
    $stream.Dispose()
    $sha.Dispose()
}
Write-Output "runtime rollback complete: $hash"
