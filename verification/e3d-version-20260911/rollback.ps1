param([string]$TargetRoot = 'D:\work\plant-code\old\gen-model')
$ErrorActionPreference = 'Stop'
$baseline = Get-Content -LiteralPath "$PSScriptRoot\baseline-hashes.json" -Raw | ConvertFrom-Json
$modified = Get-Content -LiteralPath "$PSScriptRoot\modified-hashes.json" -Raw | ConvertFrom-Json
foreach ($entry in $modified) {
    $target = Join-Path $TargetRoot $entry.path
    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $entry.sha256) {
        throw "File changed since verification: $target"
    }
}
foreach ($entry in $baseline) {
    $source = Join-Path "$PSScriptRoot\original" $entry.path
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $entry.sha256) {
        throw "Backup hash mismatch: $source"
    }
}
foreach ($entry in $baseline) {
    $target = Join-Path $TargetRoot $entry.path
    Copy-Item -LiteralPath (Join-Path "$PSScriptRoot\original" $entry.path) -Destination $target
    if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $entry.sha256) {
        throw "Restore hash mismatch: $target"
    }
}
'ROLLBACK_OK: 3 original files restored and SHA256 verified'
