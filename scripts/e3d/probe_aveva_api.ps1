# 反射 E3D 3.1 的 .NET 程序集，找出可从 C# 执行 PDMS 命令的入口。
# 必须用 Windows PowerShell 5.1（.NET Framework）跑：ReflectionOnlyLoadFrom 在 pwsh 7 上不可用。
$ErrorActionPreference = 'SilentlyContinue'
$root = 'C:\Program Files (x86)\AVEVA\Everything3D3.1'
$pattern = if ($args.Count -gt 0) { $args[0] } else { 'Command' }

$dlls = @(
    'Aveva.Core.Utilities.dll',
    'Aveva.Core.Database.dll',
    'PMLNet.dll',
    'Aveva.ApplicationFramework.dll',
    'Aveva.Core.Presentation.dll',
    'Aveva.Pdms.Utilities.dll'
)

foreach ($d in $dlls) {
    $p = Join-Path $root $d
    if (-not (Test-Path $p)) { Write-Output "MISSING $d"; continue }
    $a = $null
    try {
        $a = [Reflection.Assembly]::ReflectionOnlyLoadFrom($p)
    } catch {
        Write-Output ("LOADFAIL {0}" -f $d)
        continue
    }
    $types = @()
    try {
        $types = $a.GetTypes() | Where-Object { $_.IsPublic -and $_.FullName -match $pattern }
    } catch [System.Reflection.ReflectionTypeLoadException] {
        $types = $_.Exception.Types | Where-Object { $_ -ne $null -and $_.IsPublic -and $_.FullName -match $pattern }
    }
    Write-Output ("== {0} ({1} hits) ==" -f $d, @($types).Count)
    foreach ($t in @($types | Select-Object -First 15)) {
        Write-Output ("   " + $t.FullName)
        $ms = @()
        try { $ms = $t.GetMethods('Public,Static,Instance,DeclaredOnly') } catch {}
        foreach ($m in @($ms | Select-Object -First 8)) {
            $ps = ($m.GetParameters() | ForEach-Object { $_.ParameterType.Name }) -join ','
            Write-Output ("      ." + $m.Name + "(" + $ps + ")")
        }
    }
}
