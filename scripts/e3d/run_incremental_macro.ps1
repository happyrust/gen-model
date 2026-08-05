param(
    [Parameter(Mandatory = $true)][string]$Macro,
    [string]$Log = "D:\work\plant-code\old\gen-model\output\e3d_incremental_test.log",
    [int]$DelayMs = 20000,
    [switch]$Quit
)

$ErrorActionPreference = "Stop"
$root = "D:\work\plant-code\old\gen-model"
$shadow = "E:\reverse\e3d\shadow_e3d31_aps_all"

Copy-Item "$root\output\GenModelIncrementalTest.dll" "$shadow\GenModelIncrementalTest.dll" -Force
$env:GENMODEL_E3D_MACRO = (Resolve-Path $Macro).Path
$env:GENMODEL_E3D_LOG = $Log
$env:GENMODEL_E3D_DELAY_MS = $DelayMs.ToString()
$env:GENMODEL_E3D_QUIT = if ($Quit) { "1" } else { "0" }

& "E:\reverse\e3d\launch_e3d_sample_repaired.ps1" `
    -UseShadowInstall `
    -NoCleanup `
    -NoDllDeploy `
    -NoSensitiveBinaryRefresh `
    -NoCorePatch `
    -NoPostRepair `
    -ProjectCode ams `
    -ProjectDirectory AvevaMarineSample `
    -ProjectEnvPrefix AMS `
    -Mdb /ALL1
