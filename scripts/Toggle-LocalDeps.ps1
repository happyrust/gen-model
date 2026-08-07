# Toggle the LOCAL-DEPS PATCH block in Cargo.toml between commented (off) and active (on).
# Usage: powershell -File scripts\Toggle-LocalDeps.ps1 [-On|-Off|-Status]
param(
    [switch]$On,
    [switch]$Off,
    [switch]$Status
)

$ErrorActionPreference = 'Stop'

$manifest = Join-Path $PSScriptRoot '..\Cargo.toml' | Resolve-Path | Select-Object -ExpandProperty Path
$beginMark = '# LOCAL-DEPS PATCH BEGIN'
$endMark = '# LOCAL-DEPS PATCH END'

$lines = [System.IO.File]::ReadAllLines($manifest)
$begin = [Array]::FindIndex($lines, [Predicate[string]] { $args[0].Trim() -eq $beginMark })
$end = [Array]::FindIndex($lines, [Predicate[string]] { $args[0].Trim() -eq $endMark })

if ($begin -lt 0 -or $end -lt 0 -or $end -le $begin) {
    throw "Cargo.toml is missing the '$beginMark' / '$endMark' markers."
}

# Only the lines strictly between the markers are toggled; the markers stay comments.
$bodyStart = $begin + 1
$bodyEnd = $end - 1
$isOn = $false
for ($i = $bodyStart; $i -le $bodyEnd; $i++) {
    if ($lines[$i].Trim() -like '`[patch.*') { $isOn = $true; break }
}

if ($Status -or (-not $On -and -not $Off)) {
    if ($isOn) {
        Write-Output "local deps patch: ON  (Cargo.toml redirects aios_core / parse_pdms_db / pdms_io to local clones)"
        Write-Output "                     do NOT push this state to main - run this script with -Off first"
    }
    else {
        Write-Output "local deps patch: OFF (all three crates come from their git sources)"
    }
    exit 0
}

if ($On -and $Off) { throw "Pass either -On or -Off, not both." }

if ($On) {
    if ($isOn) { Write-Output "already ON, nothing to do"; exit 0 }
    for ($i = $bodyStart; $i -le $bodyEnd; $i++) {
        $lines[$i] = $lines[$i] -replace '^(\s*)# ?', '$1'
    }
    $verb = 'ON'
}
else {
    if (-not $isOn) { Write-Output "already OFF, nothing to do"; exit 0 }
    for ($i = $bodyStart; $i -le $bodyEnd; $i++) {
        if ($lines[$i].Trim() -eq '') { $lines[$i] = '#' } else { $lines[$i] = '# ' + $lines[$i] }
    }
    $verb = 'OFF'
}

[System.IO.File]::WriteAllLines($manifest, $lines)
Write-Output "local deps patch: $verb"

# Cargo.lock has to follow: turning the patch on drops the `source` lines for the
# three crates, turning it off puts them back. Leaving the lock stale would make
# the next build (or the pre-push guard) disagree with the manifest.
Push-Location (Split-Path $manifest -Parent)
try {
    cargo metadata --format-version 1 > $null
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed - Cargo.lock was not refreshed" }
}
finally {
    Pop-Location
}
Write-Output "Cargo.lock refreshed"

if ($On) {
    Write-Output ""
    Write-Output "reminder: run this script with -Off before pushing to main"
}
