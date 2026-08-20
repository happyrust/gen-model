param([string]$Target = 'D:\work\plant-code\old\gen-model\src\bin\node_gen_room_probe.rs')
$ErrorActionPreference = 'Stop'
$before = 'D:\work\plant-code\old\gen-model\docs\evidence\2026-08-20-room-probe-edge-traversal\node_gen_room_probe.before.rs'
if (-not (Test-Path -LiteralPath $Target)) { throw "Target missing: $Target" }
Copy-Item -LiteralPath $before -Destination $Target -Force
$hash = (Get-FileHash -LiteralPath $Target -Algorithm SHA256).Hash
if ($hash -ne '19BEBEFE731A11251B5E6168B3B0DBC95C6BE63C52C077C93FDDCD75D6F07653') { throw "Rollback hash mismatch: $hash" }
Write-Output "ROLLBACK_OK target=$Target sha256=$hash"
