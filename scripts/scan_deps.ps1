function Get-PEImports {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $e_lfanew = [BitConverter]::ToInt32($bytes, 0x3C)
    # PE signature check
    if ($bytes[$e_lfanew] -ne 0x50 -or $bytes[$e_lfanew+1] -ne 0x45) { return @() }
    $coff = $e_lfanew + 4
    $numSections = [BitConverter]::ToUInt16($bytes, $coff + 2)
    $optSize = [BitConverter]::ToUInt16($bytes, $coff + 16)
    $opt = $coff + 20
    $magic = [BitConverter]::ToUInt16($bytes, $opt)
    if ($magic -eq 0x20B) { $ddOff = $opt + 112 } else { $ddOff = $opt + 96 }
    $importRva = [BitConverter]::ToUInt32($bytes, $ddOff + 8)   # data dir index 1
    if ($importRva -eq 0) { return @() }
    # section headers start after optional header
    $secStart = $opt + $optSize
    $sections = @()
    for ($i = 0; $i -lt $numSections; $i++) {
        $s = $secStart + $i * 40
        $va = [BitConverter]::ToUInt32($bytes, $s + 12)
        $rawSize = [BitConverter]::ToUInt32($bytes, $s + 16)
        $rawPtr = [BitConverter]::ToUInt32($bytes, $s + 20)
        $sections += [PSCustomObject]@{ VA = $va; RawSize = $rawSize; RawPtr = $rawPtr }
    }
    function RvaToOff($rva, $secs) {
        foreach ($sec in $secs) {
            if ($rva -ge $sec.VA -and $rva -lt ($sec.VA + [Math]::Max($sec.RawSize,1))) {
                return $sec.RawPtr + ($rva - $sec.VA)
            }
        }
        return -1
    }
    $names = @()
    $descOff = RvaToOff $importRva $sections
    if ($descOff -lt 0) { return @() }
    while ($true) {
        $nameRva = [BitConverter]::ToUInt32($bytes, $descOff + 12)
        $origThunk = [BitConverter]::ToUInt32($bytes, $descOff + 0)
        if ($nameRva -eq 0 -and $origThunk -eq 0) { break }
        if ($nameRva -ne 0) {
            $nOff = RvaToOff $nameRva $sections
            if ($nOff -ge 0) {
                $sb = New-Object System.Text.StringBuilder
                while ($bytes[$nOff] -ne 0) { [void]$sb.Append([char]$bytes[$nOff]); $nOff++ }
                $names += $sb.ToString()
            }
        }
        $descOff += 20
    }
    return $names
}

$exeDir = Join-Path (Split-Path -Parent $PSScriptRoot) "target\debug"
$exe = Join-Path $exeDir "aios-database.exe"
$searchDirs = @($exeDir, "C:\Windows\System32", "C:\Windows\SysWOW64")
$searchDirs += ($env:PATH -split ';' | Where-Object { $_ -and (Test-Path $_) })

function Resolve-Dll($name, $dirs) {
    if ($name -match '^(api-ms-win|ext-ms-win)') { return $true } # apiset, virtual
    foreach ($d in $dirs) {
        if (Test-Path (Join-Path $d $name)) { return $true }
    }
    return $false
}

$visited = @{}
$queue = New-Object System.Collections.Queue
$queue.Enqueue($exe)
$visited[[System.IO.Path]::GetFileName($exe).ToLower()] = $true
$missing = @{}

while ($queue.Count -gt 0) {
    $cur = $queue.Dequeue()
    if (-not (Test-Path $cur)) { continue }
    $imports = Get-PEImports $cur
    foreach ($imp in $imports) {
        $lower = $imp.ToLower()
        # resolve
        $ok = Resolve-Dll $imp $searchDirs
        if (-not $ok) {
            if (-not $missing.ContainsKey($lower)) { $missing[$lower] = @() }
            $missing[$lower] += [System.IO.Path]::GetFileName($cur)
            continue
        }
        # find full path to recurse (skip system dlls to keep it bounded)
        if ($visited.ContainsKey($lower)) { continue }
        $visited[$lower] = $true
        # recurse into any DLL that we ship in the exe dir (local, non-system deps)
        $localFull = Join-Path $exeDir $imp
        if (Test-Path $localFull) { $queue.Enqueue($localFull) }
    }
}

Write-Output "=== MISSING DLLs (name <- importers) ==="
if ($missing.Count -eq 0) { Write-Output "(none missing via exe-dir+System32+PATH search)" }
foreach ($k in $missing.Keys) {
    $importers = ($missing[$k] | Select-Object -Unique) -join ', '
    Write-Output "$k   <- $importers"
}
