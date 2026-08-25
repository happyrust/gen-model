[CmdletBinding()]
param(
    [string]$PlantUiRoot = 'D:\work\plant-code\old\plant-ui-ams8000-convergence',
    [string]$Executable = 'D:\Rust\target\debug\plant-ui-app.exe',
    [switch]$NoInspection
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dbOption = [System.IO.Path]::GetFullPath((Join-Path $repo '.sites\8000\DbOption'))
$meshDir = [System.IO.Path]::GetFullPath((Join-Path $repo '.sites\8000\assets\meshes'))
$assetRoot = [System.IO.Path]::GetFullPath((Join-Path $PlantUiRoot 'web\public\assets'))
$settingsFile = [System.IO.Path]::GetFullPath((Join-Path $repo '.sites\8000\plant-ui-settings.ron'))

foreach ($item in @(
    @{ Name = 'Plant UI executable'; Path = $Executable; Type = 'Leaf' },
    @{ Name = 'AMS8000 DbOption'; Path = "$dbOption.toml"; Type = 'Leaf' },
    @{ Name = 'AMS8000 mesh directory'; Path = $meshDir; Type = 'Container' },
    @{ Name = 'Plant UI asset root'; Path = $assetRoot; Type = 'Container' }
)) {
    if (-not (Test-Path -LiteralPath $item.Path -PathType $item.Type)) {
        throw "$($item.Name) does not exist: $($item.Path)"
    }
}

# Use an isolated settings file so a previous 7997/8009 UI session cannot
# override this run's model API or mesh directory. The asset root deliberately
# has no config/e3d.project.ron: AMS8000 uses the current DbOption path only.
$env:DB_OPTION_FILE = $dbOption
$env:PLANT_ASSET_ROOT = $assetRoot
$env:PLANT_MESH_DIR = $meshDir
$env:PLANT_UI_SETTINGS_FILE = $settingsFile
if ($NoInspection) {
    Remove-Item Env:EGUI_INSPECTION -ErrorAction SilentlyContinue
} else {
    $env:EGUI_INSPECTION = '1'
}

$process = Start-Process -FilePath $Executable -WorkingDirectory $PlantUiRoot -PassThru
Write-Host "PLANT_UI_AMS8000_PID=$($process.Id)"
Write-Host "PLANT_UI_AMS8000_DB_OPTION=$dbOption"
Write-Host "PLANT_UI_AMS8000_MESH_DIR=$meshDir"
Write-Host "PLANT_UI_AMS8000_ASSET_ROOT=$assetRoot"
Write-Host "PLANT_UI_AMS8000_SETTINGS=$settingsFile"
