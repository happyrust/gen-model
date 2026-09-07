$ErrorActionPreference='Stop'
$uiExe=Join-Path (Split-Path -Parent $PSScriptRoot) 'output/rocksdb-8000-20260905/ui/plant-ui-app.exe'
foreach ($uiProcess in (Get-Process plant-ui-app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $uiExe })) {
    Stop-Process -Id $uiProcess.Id
    $uiProcess.WaitForExit()
}
Copy-Item -LiteralPath 'D:\Rust\target\debug\plant-ui-app.exe' -Destination $uiExe
& "$PSScriptRoot/Start-Rocks8000PlantUi.ps1"
