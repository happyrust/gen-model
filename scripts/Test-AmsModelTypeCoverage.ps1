[CmdletBinding()]
param(
    [string]$Endpoint = 'ws://127.0.0.1:8009',
    [string]$Namespace = '1516',
    [string]$Database = 'AvevaMarineSample',
    [switch]$RequireVerified
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content (Join-Path $repo 'scripts/e3d/ams_model_type_cases.json') -Raw |
    ConvertFrom-Json
$sql = 'RETURN array::sort(array::distinct(SELECT VALUE in.noun FROM inst_relate));'
$actual = ($sql | & (Join-Path $repo 'bin/surreal.exe') sql -e $Endpoint -u root -p root `
        --ns $Namespace --db $Database --json --hide-welcome | ConvertFrom-Json)[0]
if ($LASTEXITCODE) { throw 'Failed to query AMS model nouns' }

$duplicate = $manifest | Group-Object noun | Where-Object Count -ne 1 | ForEach-Object Name
$missing = $actual | Where-Object { $_ -notin $manifest.noun }
$stale = $manifest.noun | Where-Object { $_ -notin $actual }
$pending = $manifest | Where-Object coverage -ne 'verified' | ForEach-Object noun

Write-Host "AMS model nouns: actual=$($actual.Count) manifest=$($manifest.Count) verified=$($manifest.Count - $pending.Count) pending=$($pending.Count)"
if ($missing) { Write-Host "Missing: $($missing -join ', ')" }
if ($stale) { Write-Host "Stale: $($stale -join ', ')" }
if ($duplicate) { Write-Host "Duplicate: $($duplicate -join ', ')" }
if ($pending) { Write-Host "Pending: $($pending -join ', ')" }

if ($missing -or $stale -or $duplicate -or ($RequireVerified -and $pending)) { exit 1 }
