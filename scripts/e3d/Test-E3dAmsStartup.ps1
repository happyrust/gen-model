[CmdletBinding()]
param(
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$ProjectName = 'AvevaMarineSample',
    [string]$InstallDir = 'D:\work\plant-code\old\e3d31-runtime',
    [string]$Dbnums = '8000,7998',
    [string]$Output = ''
)

$ErrorActionPreference = 'Stop'
function Resolve-Existing([string]$Path, [string]$Label, [switch]$Directory) {
    $kind = if ($Directory) { 'Container' } else { 'Leaf' }
    if (-not (Test-Path -LiteralPath $Path -PathType $kind)) { throw "$Label 不存在：$Path" }
    return (Resolve-Path -LiteralPath $Path).Path
}
$project = Resolve-Existing $ProjectDir 'E3D 项目目录' -Directory
$install = Resolve-Existing $InstallDir 'E3D runtime' -Directory
$des = Resolve-Existing (Join-Path $install 'des.exe') 'des.exe'
$evars = Resolve-Existing (Join-Path $project ("evars{0}.bat" -f $ProjectName)) '项目 evars'
$ams000 = Resolve-Existing (Join-Path $project 'ams000') 'ams000' -Directory
foreach ($required in @('amscom', 'amssys')) { [void](Resolve-Existing (Join-Path $ams000 $required) "ams000\$required") }
$dbs = foreach ($dbnum in ($Dbnums -split ',' | ForEach-Object { [int]$_.Trim() } | Select-Object -Unique)) {
    $path = Join-Path $ams000 ("ams{0}_0001" -f $dbnum)
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { [pscustomobject]@{ dbnum = $dbnum; path = $path; exists = $false; length = 0 }; continue }
    $item = Get-Item -LiteralPath $path
    [pscustomobject]@{ dbnum = $dbnum; path = $item.FullName; exists = $true; length = $item.Length }
}
$desProcesses = @(Get-CimInstance Win32_Process -Filter "Name='des.exe'" -ErrorAction SilentlyContinue | Select-Object ProcessId, ExecutablePath, CreationDate, CommandLine)
$result = [ordered]@{
    status = if ($dbs.exists -contains $false) { 'blocked' } else { 'ready' }
    project_name = $ProjectName; project_dir = $project; project_evars = $evars
    install_dir = $install; des_exe = $des; ams000 = $ams000; databases = @($dbs)
    running_des = @($desProcesses); checked_at = (Get-Date).ToString('o')
    note = '只读检查；不会启动、终止或写入 E3D。running_des 只作为并发启动门禁。'
}
if ($Output) { $parent = Split-Path -Parent $Output; if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }; $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Output -Encoding UTF8 }
$result | ConvertTo-Json -Depth 8
if ($result.status -eq 'blocked') { exit 2 }
