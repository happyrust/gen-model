function Get-PEImports {
    param([string]$Path)
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $e_lfanew = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($bytes[$e_lfanew] -ne 0x50 -or $bytes[$e_lfanew+1] -ne 0x45) { return @() }
    $coff = $e_lfanew + 4
    $numSections = [BitConverter]::ToUInt16($bytes, $coff + 2)
    $optSize = [BitConverter]::ToUInt16($bytes, $coff + 16)
    $opt = $coff + 20
    $magic = [BitConverter]::ToUInt16($bytes, $opt)
    if ($magic -eq 0x20B) { $ddOff = $opt + 112 } else { $ddOff = $opt + 96 }
    $importRva = [BitConverter]::ToUInt32($bytes, $ddOff + 8)
    if ($importRva -eq 0) { return @() }
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
Write-Output "=== EXE direct TK* imports ==="
$exeImports = Get-PEImports $exe
$exeImports | Where-Object { $_ -match '^TK' } | Sort-Object | ForEach-Object { Write-Output $_ }
Write-Output ""
Write-Output "=== Which exe-dir DLLs import TKService.dll ==="
Get-ChildItem $exeDir -Filter "TK*.dll" | ForEach-Object {
    $imps = Get-PEImports $_.FullName
    if ($imps -contains 'TKService.dll') { Write-Output ("{0} imports TKService.dll" -f $_.Name) }
}
Write-Output ""
Write-Output "=== Which exe-dir DLLs import jemalloc.dll ==="
Get-ChildItem $exeDir -Filter "TK*.dll" | ForEach-Object {
    $imps = Get-PEImports $_.FullName
    if ($imps -contains 'jemalloc.dll') { Write-Output ("{0} imports jemalloc.dll" -f $_.Name) }
}
