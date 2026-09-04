param(
    [string]$Capture = 'D:\Rust\target\release\e3d_tty_attlist_capture.exe',
    [int]$ChunkSize = 10
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$logs = Join-Path $here 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null

$refnos = @(Get-Content -LiteralPath (Join-Path $here 'refnos.txt') |
    Where-Object { $_ -and -not $_.StartsWith('#') })
for ($offset = 0; $offset -lt $refnos.Count; $offset += $ChunkSize) {
    $end = [Math]::Min($offset + $ChunkSize - 1, $refnos.Count - 1)
    $chunk = Join-Path $here ('chunk-{0:d2}-refnos.txt' -f ($offset / $ChunkSize))
    $refnos[$offset..$end] | Out-File -LiteralPath $chunk -Encoding ascii
    & $Capture --project AMS --mdb /ALL --refno-file $chunk --out-dir $logs
    if ($LASTEXITCODE -ne 0) {
        throw "TTY capture failed for $chunk with exit $LASTEXITCODE"
    }
}

