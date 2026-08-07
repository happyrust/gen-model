# Fail if a commit redirects the self-developed crates to local paths.
# Used by .githooks/pre-push to keep the LOCAL-DEPS PATCH block out of main.
# Usage: powershell -File scripts\Assert-NoLocalPatch.ps1 [-Ref HEAD]
param(
    [string]$Ref = 'HEAD'
)

$ErrorActionPreference = 'Stop'
$patchedCrates = @('aios_core', 'parse_pdms_db', 'pdms_io')
$problems = @()

function Get-Blob([string]$path) {
    $text = & git show "${Ref}:$path" 2>&1
    if ($LASTEXITCODE -ne 0) { throw "cannot read $path at ${Ref}: $text" }
    return $text
}

# 1. An active [patch.*] section pointing at a path is the direct symptom.
$inPatch = $false
foreach ($line in (Get-Blob 'Cargo.toml')) {
    if ($line -match '^\s*#') { continue }
    if ($line -match '^\s*\[') { $inPatch = $line -match '^\s*\[patch\.' ; continue }
    if ($inPatch -and $line -match 'path\s*=\s*"') {
        $problems += "Cargo.toml has an active [patch] redirect to a local path: $($line.Trim())"
    }
}

# 2. Even without the manifest section, a lock produced while patched loses the
#    `source` line for the patched crates, which silently pins a local checkout.
$lockLines = Get-Blob 'Cargo.lock'
$current = $null
$sourceSeen = @{}
foreach ($line in $lockLines) {
    if ($line -match '^name = "(.+)"$') { $current = $Matches[1]; continue }
    if ($line -match '^source = ' -and $null -ne $current) { $sourceSeen[$current] = $true }
}
foreach ($crate in $patchedCrates) {
    if ($lockLines -contains "name = `"$crate`"" -and -not $sourceSeen.ContainsKey($crate)) {
        $problems += "Cargo.lock pins '$crate' with no source - it was resolved through a local path patch"
    }
}

if ($problems.Count -gt 0) {
    Write-Host "local-dependency patch is still active at ${Ref}:" -ForegroundColor Red
    $problems | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    Write-Host ""
    Write-Host "run: powershell -File scripts\Assert-NoLocalPatch.ps1 -Ref $Ref   (to recheck)"
    Write-Host "fix: powershell -File scripts\Toggle-LocalDeps.ps1 -Off   then amend/commit Cargo.toml + Cargo.lock"
    exit 1
}

Write-Output "no local-dependency patch at ${Ref}"
exit 0
