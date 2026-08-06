$ErrorActionPreference = 'SilentlyContinue'
$a = [Reflection.Assembly]::ReflectionOnlyLoadFrom('C:\Program Files (x86)\AVEVA\Everything3D3.1\Aveva.Core.Utilities.dll')
$t = $a.GetType('Aveva.Core.Utilities.CommandLine.Command')
Write-Output ("type: {0}  abstract={1} sealed={2}" -f $t.FullName, $t.IsAbstract, $t.IsSealed)
foreach ($m in ($t.GetMethods('Public,Static,Instance,DeclaredOnly') | Sort-Object Name)) {
    $ps = ($m.GetParameters() | ForEach-Object { $_.ParameterType.Name + ' ' + $_.Name }) -join ', '
    $mod = if ($m.IsStatic) { 'static' } else { '      ' }
    Write-Output ("  {0} {1} {2}({3})" -f $mod, $m.ReturnType.Name, $m.Name, $ps)
}
