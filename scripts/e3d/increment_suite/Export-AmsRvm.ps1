[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$DbFile = '',
    [int]$Dbnum = 8000,
    [string]$Element = '/1-LNR-Q005-PJ',
    [string]$Label = 'before',
    [string]$Output = '',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$E3dInstallDir = 'D:\work\plant-code\old\e3d31-runtime',
    [string]$ProjectEvar = '',
    [string]$MainProject = 'AvevaMarineSample',
    [string]$E3dProject = 'AMS',
    [string]$E3dMdb = '/ALL',
    [switch]$AllowExistingE3dSession
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
. (Join-Path $repo 'scripts\e3d\LinkageE3dCommon.ps1')
if (-not $DbFile) { $DbFile = Join-Path $ProjectDir ("ams000\ams{0}_0001" -f $Dbnum) }
if (-not $ProjectEvar) { $ProjectEvar = Join-Path $ProjectDir 'evarsAvevaMarineSample.bat' }
if (-not $Output) { $Output = Join-Path $repo ("output\ams-increment-in-place\rvm-{0}" -f (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force -Path $Output | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
$rvm = Join-Path $Output ("rvm-{0}.rvm" -f $Label)
$log = Join-Path $Output ("rvm-{0}.log" -f $Label)
$macro = Join-Path $Output ("rvm-{0}.mac" -f $Label)
$driver = Join-Path $Output 'driver'
New-RvmExportMacro -Path $macro -Element $Element -RvmFile $rvm -LogFile $log -Note "AMS in-place RVM $Label $Element" -QueryRef
$text = Get-Content -LiteralPath $macro -Raw -Encoding UTF8
$text = $text.Replace("`$P LINKAGE-RVM-EXPORT-BEGIN`n", "`$P LINKAGE-RVM-EXPORT-BEGIN`n/1RX03-EQUI`nOWNER`n")
[IO.File]::WriteAllText($macro, $text, [Text.UTF8Encoding]::new($false))
$savedInstall = $env:L3_E3D_INSTALL_DIR
$savedProjects = $env:L3_E3D_PROJECTS_DIR
$savedAllow = $env:L3_ALLOW_EXISTING_E3D_SESSION
$env:L3_E3D_INSTALL_DIR = $E3dInstallDir
$env:L3_E3D_PROJECTS_DIR = Split-Path $ProjectDir -Parent
if ($AllowExistingE3dSession) { $env:L3_ALLOW_EXISTING_E3D_SESSION = '1' }
try {
    Invoke-TtyMacro -Repo $repo -L3Exe $L3Exe -Macro $macro -ProjectsDir (Split-Path $ProjectDir -Parent) -MainProject $MainProject `
        -InstallDir $E3dInstallDir -Evidence $driver -LogFile $log -DoneMarker 'LINKAGE-RVM-EXPORT-DONE' `
        -E3dProject $E3dProject -E3dMdb $E3dMdb -ProjectEvar $ProjectEvar
} finally {
    $env:L3_E3D_INSTALL_DIR = $savedInstall
    $env:L3_E3D_PROJECTS_DIR = $savedProjects
    $env:L3_ALLOW_EXISTING_E3D_SESSION = $savedAllow
}
if (-not (Test-Path -LiteralPath $rvm) -or (Get-Item -LiteralPath $rvm).Length -le 0) { throw "RVM 未生成：$rvm" }
[ordered]@{ label=$Label; element=$Element; dbnum=$Dbnum; rvm=$rvm; log=$log; macro=$macro; exported_at=(Get-Date).ToString('o') } |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Output 'rvm-export.json') -Encoding UTF8
Write-Host "RVM：$rvm"
