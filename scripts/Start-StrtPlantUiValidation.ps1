param(
    [ValidateSet('backend','ui')][string]$Target = 'backend',
    [string]$BackendExe = '',
    [string]$WatchDbnums = '7997'
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$run = Join-Path $repo 'output/strt-cpv-20260905/live'
$env:DB_OPTION_FILE = Join-Path $run 'DbOption'
$env:AIOS_STORE_MODE = 'embedded-mem'
$env:AIOS_STARTUP_AUTORUN = '0'
$env:AIOS_DATA_INCREMENTAL = '0'
$env:AIOS_MODEL_INCREMENTAL = '0'
$env:AIOS_ROOM_INCREMENTAL = '0'
$env:RUST_MIN_STACK = '134217728'
if ($Target -eq 'backend') {
    $exe = Join-Path $repo 'output/strt-cpv-20260905/aios-database.exe'
    if ($BackendExe) { $exe = (Resolve-Path -LiteralPath $BackendExe).Path }
    $p = Start-Process -FilePath $exe -ArgumentList 'serve','--watch-dbnum',$WatchDbnums -WorkingDirectory $run -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $run 'backend.stdout.log') -RedirectStandardError (Join-Path $run 'backend.stderr.log')
} else {
    $ui = Join-Path $run 'ui'
    $env:PLANT_UI_SETTINGS_FILE = Join-Path $ui 'config/settings.ron'
    $env:PLANT_MODEL_API_URL = 'http://127.0.0.1:18206'
    $env:PLANT_ASSET_ROOT = Join-Path $ui 'assets'
    $env:EGUI_INSPECTION = '1'
    $p = Start-Process -FilePath (Join-Path $ui 'plant-ui-app.exe') -WorkingDirectory $ui -PassThru -RedirectStandardOutput (Join-Path $run 'ui.stdout.log') -RedirectStandardError (Join-Path $run 'ui.stderr.log')
}
$p.Id | Set-Content (Join-Path $run "$Target.pid")
Write-Output "$Target PID $($p.Id)"
