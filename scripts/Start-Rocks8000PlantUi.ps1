$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$run=Join-Path $repo 'output/rocksdb-8000-20260905'
$ui=Join-Path $run 'ui'
$source=Join-Path $repo 'output/pipe26229-live-sandbox-20260905/ui'
if (!(Test-Path $ui)) {
    New-Item -ItemType Directory $ui | Out-Null
    Copy-Item (Join-Path $source 'plant-ui-app.exe') $ui
    Copy-Item (Join-Path $source 'assets') $ui -Recurse
    Copy-Item (Join-Path $run 'DbOption.toml') $ui
}
$config=@'
(
 api_host:"http://127.0.0.1:18050",
 db_host:"ws://127.0.0.1:18049",
 mdb_name:"ALL", project_name:"AvevaMarineSample",
 project_code:"rocks8000_20260905", module:"DESI", auto_gen_mesh:true,
)
'@
[IO.File]::WriteAllText((Join-Path $ui 'assets/config/e3d.project.ron'),$config)
$env:PLANT_ASSET_ROOT=Join-Path $ui 'assets'
$env:PLANT_UI_SETTINGS_FILE=Join-Path $ui 'isolated-settings.ron'
$env:PLANT_MODEL_API_URL='http://127.0.0.1:18050'
$env:PLANT_DATA_API_URL='http://127.0.0.1:18050'
$env:PLANT_MESH_DIR=Join-Path $run 'meshes'
$env:EGUI_INSPECTION='1'
$p=Start-Process (Join-Path $ui 'plant-ui-app.exe') -WorkingDirectory $ui -WindowStyle Normal -PassThru -RedirectStandardOutput (Join-Path $run 'ui.stdout.log') -RedirectStandardError (Join-Path $run 'ui.stderr.log')
$p.Id | Set-Content (Join-Path $run 'ui.pid')
Write-Output $p.Id
