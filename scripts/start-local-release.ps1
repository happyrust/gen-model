param(
    [string]$Workspace = 'D:\work\plant-code\old\test-worklspace\bin',
    [switch]$NoBrowser,
    [int]$TimeoutSeconds = 600
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$exe = Join-Path $root 'aios-database.exe'
$url = 'http://127.0.0.1:9099/'
$logRoot = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logRoot | Out-Null
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing backend: $exe" }
if (-not (Test-Path -LiteralPath (Join-Path $root 'web/pkg/plant_ui_app_bg.wasm'))) { throw 'Web package missing' }
$existing = Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" | Where-Object { $_.ExecutablePath -eq $exe }
if ($existing) {
    $process = Get-Process -Id $existing.ProcessId
} else {
    $listener = [System.Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners() |
        Where-Object { $_.Port -eq 9099 } |
        Select-Object -First 1
    if ($listener) { throw "Port 9099 is already in use. Stop the prior release first." }
    $env:DB_OPTION_FILE = Join-Path $Workspace 'DbOption.toml'
    # 介质显式写出来：服务自己的默认 2026-09-18 起是 rocksdb（ADR-083 D6），而这个老启动器
    # 一直跑的是本地发布包那份不带库的 workspace。不写这一行，它下一次就会去连 v_ip:v_port
    # 然后启动失败。要拿它连一台真库，起之前自己设 $env:AIOS_STORE_MODE = 'rocksdb'。
    if (-not $env:AIOS_STORE_MODE) { $env:AIOS_STORE_MODE = 'mem' }
    $env:PLANT_UI_WEB_ROOT = Join-Path $root 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
    $surreal = Join-Path $Workspace 'bin\surreal.exe'
    if (Test-Path -LiteralPath $surreal) { $env:AIOS_SURREAL_EXE = $surreal }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $process = Start-Process -FilePath $exe -ArgumentList 'serve' -WorkingDirectory $Workspace -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logRoot "$stamp.stdout.log") -RedirectStandardError (Join-Path $logRoot "$stamp.stderr.log")
    @{ pid=$process.Id; start_time=$process.StartTime.ToString('o'); executable=$exe } | ConvertTo-Json | Set-Content (Join-Path $root 'process.json')
}
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    $process.Refresh()
    if ($process.HasExited) { throw "Backend exited: $($process.ExitCode); see $logRoot" }
    try {
        $health = Invoke-WebRequest -Uri ($url + 'api/v1/health') -UseBasicParsing -TimeoutSec 3
        $page = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 3
        if ($health.StatusCode -eq 200 -and $page.Content.Contains('plant_ui_app.js')) {
            Write-Output "READY PID=$($process.Id) URL=$url"
            if (-not $NoBrowser) { Start-Process $url }
            exit 0
        }
    } catch {}
    Start-Sleep -Seconds 2
} while ((Get-Date) -lt $deadline)
throw "Startup still pending; process left running. Inspect $logRoot"
