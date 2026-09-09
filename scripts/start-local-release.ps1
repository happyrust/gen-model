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
    $listener = Get-NetTCPConnection -LocalPort 9099 -State Listen -ErrorAction SilentlyContinue
    if ($listener) { throw "Port 9099 already belongs to PID $($listener.OwningProcess). Stop the prior release first." }
    $env:DB_OPTION_FILE = Join-Path $Workspace 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $root 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
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
