param([switch]$Reapply)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$patch = Join-Path $PSScriptRoot 'focused.patch'
$manifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'hashes.json') | ConvertFrom-Json
Push-Location $repo
try {
    if ($Reapply) {
        git apply --check -- $patch
        if ($LASTEXITCODE -ne 0) { throw "forward patch check failed: $LASTEXITCODE" }
        git apply -- $patch
        $field = 'modified_sha256'
    } else {
        git apply --check --reverse -- $patch
        if ($LASTEXITCODE -ne 0) { throw "reverse patch check failed: $LASTEXITCODE" }
        git apply --reverse -- $patch
        $field = 'baseline_sha256'
    }
    foreach ($prop in $manifest.PSObject.Properties) {
        $path = Join-Path $repo $prop.Name
        # Patch text is normalized to LF; compare the same logical content on Windows
        # rather than treating CRLF as a code change.
        $normalized = [IO.File]::ReadAllText($path).Replace("`r`n", "`n")
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($normalized)
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            $actual = ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
        } finally {
            $sha.Dispose()
        }
        $expected = $prop.Value.$field
        if ($actual -ne $expected) { throw "hash mismatch: $($prop.Name) expected=$expected actual=$actual" }
    }
    $fileCount = @($manifest.PSObject.Properties).Count
    "OK mode=$(@('rollback','reapply')[[int]$Reapply.IsPresent]) files=$fileCount"
} finally { Pop-Location }
