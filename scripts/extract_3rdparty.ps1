Add-Type -AssemblyName System.IO.Compression.FileSystem
$zipPath = "D:\occt-3rdparty-vc14-64.zip"
$exeDir  = Join-Path (Split-Path -Parent $PSScriptRoot) "target\debug"
$installRoot = "C:\tools"   # zip has top folder 3rdparty-vc14-64 -> C:\tools\3rdparty-vc14-64

# product folder name fragments we need (memory mgr + data-exchange/service deps)
$needed = @('jemalloc', 'freeimage', 'ffmpeg', 'openvr', 'tbb2021', 'freetype')

$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
$copiedBeside = @()
foreach ($e in $zip.Entries) {
    if ($e.FullName -notlike '*.dll') { continue }
    $lower = $e.FullName.ToLower()
    if ($lower -notmatch '/bin') { continue }
    $match = $false
    foreach ($n in $needed) { if ($lower -match [regex]::Escape($n)) { $match = $true; break } }
    if (-not $match) { continue }

    # 1) install to C:\tools\3rdparty-vc14-64\... preserving structure
    $destInstall = Join-Path $installRoot ($e.FullName -replace '/', '\')
    $destDir = Split-Path $destInstall -Parent
    if (-not (Test-Path $destDir)) { New-Item -ItemType Directory -Force -Path $destDir | Out-Null }
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $destInstall, $true)

    # 2) copy flat beside the exe
    $besideDest = Join-Path $exeDir $e.Name
    Copy-Item -Path $destInstall -Destination $besideDest -Force
    $copiedBeside += $e.Name
}
$zip.Dispose()

Write-Output "=== DLLs deployed beside exe ($($copiedBeside.Count)) ==="
$copiedBeside | Sort-Object -Unique | ForEach-Object { Write-Output $_ }
